mod arp;
mod banner;
mod icmp;
mod interface;
mod oui;
mod output;
mod portscan;
mod tui;
mod types;
mod wol;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use ipnetwork::Ipv4Network;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use types::HostInfo;

#[derive(Parser)]
#[command(name = "nd", about = "Discover live hosts on your local network")]
struct Opts {
    /// Subnet to scan (default: auto-detect from local interfaces)
    #[arg(long, value_name = "CIDR")]
    target: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value = "tui")]
    output: OutputFormat,

    /// Resolve hostnames via reverse DNS
    #[arg(long)]
    resolve: bool,

    /// Max concurrent probes
    #[arg(long, default_value = "256")]
    concurrency: usize,
}

#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
    Tui,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    check_root()?;

    if let OutputFormat::Tui = opts.output {
        return run_tui(opts).await;
    }

    let mut hosts = scan(&opts).await?;

    for host in &mut hosts {
        if let Some(mac) = host.mac {
            host.vendor = oui::lookup(mac).map(str::to_string);
        }
    }

    if opts.resolve {
        resolve_hostnames(&mut hosts, opts.concurrency).await;
    }

    match opts.output {
        OutputFormat::Table => output::print_table(&hosts),
        OutputFormat::Json => output::print_json(&hosts)?,
        OutputFormat::Tui => unreachable!(),
    }

    Ok(())
}

async fn run_tui(opts: Opts) -> Result<()> {
    let scan_targets = compute_targets(&opts)?;
    let target_str = opts
        .target
        .clone()
        .unwrap_or_else(|| "auto-detected interfaces".to_string());

    let (event_tx, event_rx) = mpsc::channel::<tui::ScanEvent>(512);

    let concurrency = opts.concurrency;
    let resolve = opts.resolve;
    let scan_handle = tokio::spawn(async move {
        scan_task(scan_targets, concurrency, resolve, event_tx).await;
    });

    tui::run(event_rx, target_str).await?;
    scan_handle.abort();
    Ok(())
}

async fn scan_task(
    scan_targets: Vec<(interface::Interface, Vec<Ipv4Addr>)>,
    concurrency: usize,
    resolve: bool,
    tx: mpsc::Sender<tui::ScanEvent>,
) {
    let mut found: HashSet<Ipv4Addr> = HashSet::new();

    // ARP phase — runs per interface, blocks for 2s each
    for (iface, ips) in &scan_targets {
        let arp_results = arp::scan(iface, ips.clone(), Duration::from_secs(2)).await;
        for (ip, mac) in arp_results {
            if found.insert(ip) {
                let vendor = oui::lookup(mac).map(str::to_string);
                let host = HostInfo { ip, mac: Some(mac), hostname: None, vendor };
                if tx.send(tui::ScanEvent::Host(host)).await.is_err() {
                    return;
                }
            }
        }
    }

    // ICMP fallback — ping hosts that didn't respond to ARP, stream results
    let remaining: Vec<Ipv4Addr> = scan_targets
        .iter()
        .flat_map(|(_, ips)| ips.iter().copied())
        .filter(|ip| !found.contains(ip))
        .collect();

    let (icmp_tx, mut icmp_rx) = mpsc::channel::<Ipv4Addr>(256);
    let sem = Arc::new(Semaphore::new(concurrency));

    tokio::spawn(async move {
        let mut handles = Vec::with_capacity(remaining.len());
        for ip in remaining {
            let sem = sem.clone();
            let tx = icmp_tx.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                if icmp::ping(ip).await {
                    tx.send(ip).await.ok();
                }
            }));
        }
        for h in handles {
            h.await.ok();
        }
        // icmp_tx dropped here, closing icmp_rx
    });

    while let Some(ip) = icmp_rx.recv().await {
        found.insert(ip);
        let host = HostInfo { ip, mac: None, hostname: None, vendor: None };
        if tx.send(tui::ScanEvent::Host(host)).await.is_err() {
            return;
        }
    }

    // Hostname resolution (opt-in) — stream updates as DNS responds
    if resolve {
        let all_ips: Vec<Ipv4Addr> = found.into_iter().collect();
        let sem = Arc::new(Semaphore::new(concurrency.min(64)));
        let (dns_tx, mut dns_rx) = mpsc::channel::<(Ipv4Addr, String)>(256);

        tokio::spawn(async move {
            let mut handles = Vec::with_capacity(all_ips.len());
            for ip in all_ips {
                let sem = sem.clone();
                let dtx = dns_tx.clone();
                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    let name = tokio::task::spawn_blocking(move || {
                        dns_lookup::lookup_addr(&IpAddr::V4(ip)).ok()
                    })
                    .await
                    .ok()
                    .flatten();
                    if let Some(n) = name {
                        dtx.send((ip, n)).await.ok();
                    }
                }));
            }
            for h in handles {
                h.await.ok();
            }
        });

        while let Some((ip, name)) = dns_rx.recv().await {
            if tx.send(tui::ScanEvent::Hostname(ip, name)).await.is_err() {
                return;
            }
        }
    }

    tx.send(tui::ScanEvent::Done).await.ok();
}

fn check_root() -> Result<()> {
    // SAFETY: geteuid() is always safe to call on Unix
    #[cfg(unix)]
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!(
            "raw socket requires root.\n  Run with: sudo nd\n  Or grant capability: sudo setcap cap_net_raw+ep $(which nd)"
        );
    }
    Ok(())
}

fn compute_targets(opts: &Opts) -> Result<Vec<(interface::Interface, Vec<Ipv4Addr>)>> {
    if let Some(target_str) = &opts.target {
        let network: Ipv4Network = target_str
            .parse()
            .with_context(|| format!("invalid CIDR: {target_str}"))?;
        if network.prefix() < 16 {
            anyhow::bail!("refusing to scan networks larger than /16");
        }
        let iface = interface::for_network(network)?;
        Ok(vec![(iface, network_hosts(network))])
    } else {
        Ok(interface::list()?
            .into_iter()
            .map(|iface| {
                let ips = network_hosts(iface.network);
                (iface, ips)
            })
            .collect())
    }
}

async fn scan(opts: &Opts) -> Result<Vec<HostInfo>> {
    let scan_targets = compute_targets(opts)?;

    // ARP sweep
    let mut found: HashMap<Ipv4Addr, Option<[u8; 6]>> = HashMap::new();
    for (iface, ips) in &scan_targets {
        let results = arp::scan(iface, ips.clone(), Duration::from_secs(2)).await;
        for (ip, mac) in results {
            found.insert(ip, Some(mac));
        }
    }

    // ICMP fallback for hosts that didn't respond to ARP
    let remaining: Vec<Ipv4Addr> = scan_targets
        .iter()
        .flat_map(|(_, ips)| ips.iter().copied())
        .filter(|ip| !found.contains_key(ip))
        .collect();

    let sem = Arc::new(Semaphore::new(opts.concurrency));
    let mut handles = Vec::with_capacity(remaining.len());
    for ip in remaining {
        let sem = sem.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            if icmp::ping(ip).await { Some(ip) } else { None }
        }));
    }
    for handle in handles {
        if let Some(ip) = handle.await? {
            found.insert(ip, None);
        }
    }

    let mut hosts: Vec<HostInfo> = found
        .into_iter()
        .map(|(ip, mac)| HostInfo { ip, mac, hostname: None, vendor: None })
        .collect();
    hosts.sort_by_key(|h| h.ip);
    Ok(hosts)
}

fn network_hosts(network: Ipv4Network) -> Vec<Ipv4Addr> {
    if network.prefix() >= 31 {
        network.iter().collect()
    } else {
        network
            .iter()
            .filter(|&ip| ip != network.network() && ip != network.broadcast())
            .collect()
    }
}

async fn resolve_hostnames(hosts: &mut Vec<HostInfo>, concurrency: usize) {
    let sem = Arc::new(Semaphore::new(concurrency.min(64)));
    let mut handles = Vec::with_capacity(hosts.len());

    for host in hosts.iter() {
        let ip = host.ip;
        let sem = sem.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let hostname = tokio::task::spawn_blocking(move || {
                dns_lookup::lookup_addr(&IpAddr::V4(ip)).ok()
            })
            .await
            .ok()
            .flatten();
            (ip, hostname)
        }));
    }

    let mut dns: HashMap<Ipv4Addr, String> = HashMap::new();
    for handle in handles {
        if let Ok((ip, Some(hostname))) = handle.await {
            dns.insert(ip, hostname);
        }
    }

    for host in hosts.iter_mut() {
        host.hostname = dns.remove(&host.ip);
    }
}
