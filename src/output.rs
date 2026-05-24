use crate::types::{HostInfo, HostInfoJson};
use anyhow::Result;
use comfy_table::Table;

pub fn print_table(hosts: &[HostInfo]) {
    let mut table = Table::new();
    table.set_header(["IP Address", "MAC Address", "Vendor", "Hostname"]);
    for host in hosts {
        table.add_row([
            host.ip.to_string(),
            host.mac_display(),
            host.vendor.as_deref().unwrap_or("").to_string(),
            host.hostname.as_deref().unwrap_or("").to_string(),
        ]);
    }
    println!("{table}");
}

pub fn print_json(hosts: &[HostInfo]) -> Result<()> {
    let json: Vec<HostInfoJson> = hosts.iter().map(HostInfoJson::from).collect();
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}
