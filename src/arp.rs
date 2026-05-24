use crate::interface::Interface;
use pnet::datalink::{self, Channel, Config};
use pnet::packet::arp::{ArpHardwareTypes, ArpOperations, ArpPacket, MutableArpPacket};
use pnet::packet::ethernet::{EtherTypes, EthernetPacket, MutableEthernetPacket};
use pnet::packet::{MutablePacket, Packet};
use pnet::util::MacAddr;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// 14 bytes ethernet header + 28 bytes ARP
const ETH_ARP_LEN: usize = 42;

pub async fn scan(
    iface: &Interface,
    targets: Vec<Ipv4Addr>,
    timeout: Duration,
) -> HashMap<Ipv4Addr, [u8; 6]> {
    let iface_inner = iface.inner.clone();
    let source_ip = iface.ip;
    let source_mac = iface.mac;

    tokio::task::spawn_blocking(move || {
        scan_blocking(iface_inner, targets, source_ip, source_mac, timeout)
    })
    .await
    .unwrap_or_default()
}

fn scan_blocking(
    iface: datalink::NetworkInterface,
    targets: Vec<Ipv4Addr>,
    source_ip: Ipv4Addr,
    source_mac: [u8; 6],
    timeout: Duration,
) -> HashMap<Ipv4Addr, [u8; 6]> {
    let config = Config {
        read_timeout: Some(Duration::from_millis(50)),
        ..Default::default()
    };

    let (mut tx, mut rx) = match datalink::channel(&iface, config) {
        Ok(Channel::Ethernet(tx, rx)) => (tx, rx),
        _ => return HashMap::new(),
    };

    let results: Arc<Mutex<HashMap<Ipv4Addr, [u8; 6]>>> = Arc::new(Mutex::new(HashMap::new()));
    let results_rx = results.clone();
    let deadline = Instant::now() + timeout;

    let rx_handle = std::thread::spawn(move || {
        while Instant::now() < deadline {
            match rx.next() {
                Ok(packet) => {
                    let Some(eth) = EthernetPacket::new(packet) else {
                        continue;
                    };
                    if eth.get_ethertype() != EtherTypes::Arp {
                        continue;
                    }
                    let Some(arp) = ArpPacket::new(eth.payload()) else {
                        continue;
                    };
                    if arp.get_operation() == ArpOperations::Reply {
                        let ip = arp.get_sender_proto_addr();
                        let m = arp.get_sender_hw_addr();
                        let mac = [m.0, m.1, m.2, m.3, m.4, m.5];
                        results_rx.lock().unwrap().insert(ip, mac);
                    }
                }
                Err(_) => {} // read timeout — check deadline on next iteration
            }
        }
    });

    let src_mac = MacAddr(
        source_mac[0],
        source_mac[1],
        source_mac[2],
        source_mac[3],
        source_mac[4],
        source_mac[5],
    );

    for target_ip in targets {
        let mut buf = [0u8; ETH_ARP_LEN];
        let mut eth = MutableEthernetPacket::new(&mut buf).unwrap();
        eth.set_destination(MacAddr::broadcast());
        eth.set_source(src_mac);
        eth.set_ethertype(EtherTypes::Arp);
        {
            let mut arp = MutableArpPacket::new(eth.payload_mut()).unwrap();
            arp.set_hardware_type(ArpHardwareTypes::Ethernet);
            arp.set_protocol_type(EtherTypes::Ipv4);
            arp.set_hw_addr_len(6);
            arp.set_proto_addr_len(4);
            arp.set_operation(ArpOperations::Request);
            arp.set_sender_hw_addr(src_mac);
            arp.set_sender_proto_addr(source_ip);
            arp.set_target_hw_addr(MacAddr::zero());
            arp.set_target_proto_addr(target_ip);
        }
        let _ = tx.send_to(eth.packet(), None);
    }

    let _ = rx_handle.join();
    Arc::try_unwrap(results).unwrap().into_inner().unwrap()
}
