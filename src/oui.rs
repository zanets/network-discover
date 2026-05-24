use std::collections::HashMap;
use std::sync::OnceLock;

static OUI_MAP: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();
static OUI_DATA: &str = include_str!("../assets/oui.txt");

fn oui_map() -> &'static HashMap<u32, &'static str> {
    OUI_MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for line in OUI_DATA.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((prefix, vendor)) = line.split_once('\t') {
                if let Ok(n) = u32::from_str_radix(prefix.trim(), 16) {
                    map.insert(n, vendor.trim());
                }
            }
        }
        map
    })
}

pub fn lookup(mac: [u8; 6]) -> Option<&'static str> {
    let oui = (mac[0] as u32) << 16 | (mac[1] as u32) << 8 | mac[2] as u32;
    oui_map().get(&oui).copied()
}
