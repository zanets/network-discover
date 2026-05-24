use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
use surge_ping::{Client, Config, PingIdentifier, PingSequence};

pub async fn ping(ip: Ipv4Addr) -> bool {
    let client = match Client::new(&Config::default()) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let id = PingIdentifier(u16::from_be_bytes([ip.octets()[2], ip.octets()[3]]));
    let mut pinger = client.pinger(IpAddr::V4(ip), id).await;
    pinger.timeout(Duration::from_secs(1));
    pinger.ping(PingSequence(0), &[]).await.is_ok()
}
