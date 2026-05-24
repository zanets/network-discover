use anyhow::{anyhow, Result};
use ipnetwork::{IpNetwork, Ipv4Network};
use pnet::datalink;
use std::net::Ipv4Addr;

pub struct Interface {
    #[allow(dead_code)]
    pub name: String,
    pub ip: Ipv4Addr,
    pub mac: [u8; 6],
    pub network: Ipv4Network,
    pub inner: datalink::NetworkInterface,
}

pub fn list() -> Result<Vec<Interface>> {
    let mut result = Vec::new();

    for iface in datalink::interfaces() {
        if iface.is_loopback() || iface.mac.is_none() {
            continue;
        }
        let m = iface.mac.unwrap();
        let mac = [m.0, m.1, m.2, m.3, m.4, m.5];

        for ip_net in &iface.ips {
            if let IpNetwork::V4(v4) = ip_net {
                let octets = v4.ip().octets();
                if octets[0] == 169 && octets[1] == 254 {
                    continue; // skip link-local
                }
                result.push(Interface {
                    name: iface.name.clone(),
                    ip: v4.ip(),
                    mac,
                    network: *v4,
                    inner: iface.clone(),
                });
            }
        }
    }

    if result.is_empty() {
        return Err(anyhow!("no suitable network interfaces found"));
    }
    Ok(result)
}

pub fn for_network(target: Ipv4Network) -> Result<Interface> {
    list()?
        .into_iter()
        .find(|iface| {
            iface.network.network() == target.network()
                || iface.network.contains(target.ip())
        })
        .ok_or_else(|| anyhow!("no local interface found for {}", target))
}
