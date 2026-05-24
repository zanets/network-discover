use serde::Serialize;
use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub struct HostInfo {
    pub ip: Ipv4Addr,
    pub mac: Option<[u8; 6]>,
    pub hostname: Option<String>,
    pub vendor: Option<String>,
}

impl HostInfo {
    pub fn mac_display(&self) -> String {
        self.mac.map_or(String::new(), |m| {
            format!(
                "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            )
        })
    }
}

#[derive(Serialize)]
pub struct HostInfoJson {
    pub ip: String,
    pub mac: Option<String>,
    pub hostname: Option<String>,
    pub vendor: Option<String>,
}

impl From<&HostInfo> for HostInfoJson {
    fn from(h: &HostInfo) -> Self {
        let mac = h.mac_display();
        Self {
            ip: h.ip.to_string(),
            mac: if mac.is_empty() { None } else { Some(mac) },
            hostname: h.hostname.clone(),
            vendor: h.vendor.clone(),
        }
    }
}
