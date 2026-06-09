use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

// ── SSDP (UPnP) Discovery ──────────────────────────────────────────────────────

async fn probe_ssdp(ip: Ipv4Addr) -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").await.ok()?;
    let request = "M-SEARCH * HTTP/1.1\r\n\
                   HOST: 239.255.255.250:1900\r\n\
                   MAN: \"ssdp:discover\"\r\n\
                   MX: 1\r\n\
                   ST: ssdp:all\r\n\
                   \r\n";
    
    // Send unicast SSDP M-SEARCH directly to the target host
    socket.send_to(request.as_bytes(), (ip, 1900)).await.ok()?;
    
    let mut buf = [0u8; 1024];
    let (len, _) = timeout(Duration::from_millis(600), socket.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;
    
    let reply = String::from_utf8_lossy(&buf[..len]);
    
    // Look for LOCATION header
    let mut location = None;
    for line in reply.lines() {
        let line_lower = line.to_lowercase();
        if line_lower.starts_with("location:") {
            location = Some(line[9..].trim().to_string());
            break;
        }
    }
    
    let url = location?;
    let (xml_ip, port, path) = parse_url(&url)?;
    fetch_xml_friendly_name(xml_ip, port, &path).await
}

fn parse_url(url: &str) -> Option<(Ipv4Addr, u16, String)> {
    let s = url.strip_prefix("http://")?;
    let (host_port, path) = s.split_once('/')?;
    let (host, port_str) = if let Some((h, p)) = host_port.split_once(':') {
        (h, p)
    } else {
        (host_port, "80")
    };
    let ip: Ipv4Addr = host.parse().ok()?;
    let port: u16 = port_str.parse().ok()?;
    Some((ip, port, format!("/{}", path)))
}

async fn fetch_xml_friendly_name(ip: Ipv4Addr, port: u16, path: &str) -> Option<String> {
    let mut stream = timeout(Duration::from_millis(600), TcpStream::connect((ip, port)))
        .await
        .ok()?
        .ok()?;
    
    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: nd/0.1\r\nConnection: close\r\n\r\n",
        path, ip
    );
    stream.write_all(req.as_bytes()).await.ok()?;
    
    let mut buf = Vec::new();
    let _ = timeout(Duration::from_millis(600), stream.take(8192).read_to_end(&mut buf)).await;
    let body = String::from_utf8_lossy(&buf);
    
    // Find <friendlyName>...</friendlyName>
    if let Some(start_idx) = body.find("<friendlyName>") {
        if let Some(end_idx) = body[start_idx..].find("</friendlyName>") {
            let name = &body[start_idx + 14..start_idx + end_idx];
            return Some(name.trim().to_string());
        }
    }
    
    // Fallback: Find <modelName>...</modelName>
    if let Some(start_idx) = body.find("<modelName>") {
        if let Some(end_idx) = body[start_idx..].find("</modelName>") {
            let name = &body[start_idx + 11..start_idx + end_idx];
            return Some(name.trim().to_string());
        }
    }
    
    None
}

// ── mDNS (DNS-SD) Discovery ────────────────────────────────────────────────────

fn encode_dns_query(service: &str) -> Vec<u8> {
    let mut packet = Vec::new();
    // Transaction ID (arbitrary)
    packet.extend_from_slice(&[0x12, 0x34]);
    // Flags: standard query, recursion desired (0x0100)
    packet.extend_from_slice(&[0x01, 0x00]);
    // Questions: 1
    packet.extend_from_slice(&[0x00, 0x01]);
    // Answer RRs, Authority RRs, Additional RRs: 0
    packet.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // Encode name labels
    for part in service.split('.') {
        if !part.is_empty() {
            packet.push(part.len() as u8);
            packet.extend_from_slice(part.as_bytes());
        }
    }
    packet.push(0); // Null terminator
    
    // Type: PTR (0x000c)
    packet.extend_from_slice(&[0x00, 0x0c]);
    // Class: IN (0x0001)
    packet.extend_from_slice(&[0x00, 0x01]);
    
    packet
}

struct ResourceRecord {
    rr_type: u16,
    rdata_absolute_offset: usize,
}

fn read_name(buf: &[u8], mut offset: usize) -> Option<(String, usize)> {
    let mut parts = Vec::new();
    let mut jumped = false;
    let mut jump_offset = 0;
    let mut visited = std::collections::HashSet::new();
    
    loop {
        if offset >= buf.len() { return None; }
        if !visited.insert(offset) { return None; }
        
        let len = buf[offset];
        if len == 0 {
            offset += 1;
            break;
        } else if (len & 0xc0) == 0xc0 {
            if offset + 1 >= buf.len() { return None; }
            let pointer = (((len & 0x3f) as usize) << 8) | (buf[offset + 1] as usize);
            if !jumped {
                jumped = true;
                jump_offset = offset + 2;
            }
            offset = pointer;
        } else {
            if offset + 1 + len as usize > buf.len() { return None; }
            let s = std::str::from_utf8(&buf[offset + 1..offset + 1 + len as usize]).ok()?;
            parts.push(s.to_string());
            offset += 1 + len as usize;
        }
    }
    
    let final_offset = if jumped { jump_offset } else { offset };
    Some((parts.join("."), final_offset))
}

fn parse_records(buf: &[u8]) -> Option<Vec<ResourceRecord>> {
    if buf.len() < 12 { return None; }
    let questions = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let answers = u16::from_be_bytes([buf[6], buf[7]]) as usize;
    let authorities = u16::from_be_bytes([buf[8], buf[9]]) as usize;
    let additionals = u16::from_be_bytes([buf[10], buf[11]]) as usize;
    
    let mut offset = 12;
    
    // Skip questions
    for _ in 0..questions {
        let (_, next_offset) = read_name(buf, offset)?;
        offset = next_offset + 4; // Skip type (2B) and class (2B)
    }
    
    let total_rrs = answers + authorities + additionals;
    let mut records = Vec::with_capacity(total_rrs);
    
    for _ in 0..total_rrs {
        if offset >= buf.len() { break; }
        let (_, next_offset) = read_name(buf, offset)?;
        offset = next_offset;
        if offset + 10 > buf.len() { break; }
        let rr_type = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let rd_len = u16::from_be_bytes([buf[offset + 8], buf[offset + 9]]) as usize;
        offset += 10;
        if offset + rd_len > buf.len() { break; }
        records.push(ResourceRecord {
            rr_type,
            rdata_absolute_offset: offset,
        });

        offset += rd_len;
    }

    Some(records)
}

async fn probe_mdns(ip: Ipv4Addr) -> Option<String> {
    // Reverse PTR lookup via unicast mDNS — macOS and Linux/Avahi respond to this
    let [a, b, c, d] = ip.octets();
    let reverse_name = format!("{}.{}.{}.{}.in-addr.arpa", d, c, b, a);

    let socket = UdpSocket::bind("0.0.0.0:0").await.ok()?;
    let query = encode_dns_query(&reverse_name);
    socket.send_to(&query, (ip, 5353)).await.ok()?;

    let mut buf = [0u8; 512];
    let (len, _) = timeout(Duration::from_millis(500), socket.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;

    let packet = &buf[..len];
    let rrs = parse_records(packet)?;

    for rr in &rrs {
        if rr.rr_type == 12 {
            if let Some((name, _)) = read_name(packet, rr.rdata_absolute_offset) {
                let hostname = name
                    .trim_end_matches('.')
                    .strip_suffix(".local")
                    .unwrap_or(name.trim_end_matches('.'));
                if !hostname.is_empty() && !hostname.starts_with('_') {
                    return Some(hostname.to_string());
                }
            }
        }
    }

    None
}

// ── NetBIOS Name Service (NBNS) ───────────────────────────────────────────────

async fn probe_nbns(ip: Ipv4Addr) -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").await.ok()?;

    // Node Status Request: wildcard "*" encoded as NBT name
    // '*' = 0x2A → high nibble 0x02 → 'C', low nibble 0x0A → 'K'
    // remaining 15 null bytes → 30 × 'A'
    let request: &[u8] = &[
        0x12, 0x34, // Transaction ID
        0x00, 0x10, // Flags: Node Status Request
        0x00, 0x01, // Questions: 1
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Answer/Authority/Additional RRs: 0
        0x20, // NBT name length (32)
        b'C', b'K', // '*' encoded
        b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A',
        b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A',
        b'A', b'A', b'A', b'A', b'A', b'A', b'A', b'A',
        b'A', b'A', b'A', b'A', b'A', b'A', // 30 × 'A'
        0x00, // end of name
        0x00, 0x21, // Type: NBSTAT
        0x00, 0x01, // Class: IN
    ];

    socket.send_to(request, (ip, 137)).await.ok()?;

    let mut buf = [0u8; 1024];
    let (len, _) = timeout(Duration::from_millis(500), socket.recv_from(&mut buf))
        .await
        .ok()?
        .ok()?;

    // Standard NBSTAT response layout:
    //   12B header + 38B question + 2B name-ptr + 2+2+4+2B RR header = RDATA at 62
    if len < 63 {
        return None;
    }
    let num_names = buf[62] as usize;
    if num_names == 0 || len < 63 + num_names * 18 {
        return None;
    }

    for i in 0..num_names {
        let base = 63 + i * 18;
        let suffix = buf[base + 15];
        let flags = u16::from_be_bytes([buf[base + 16], buf[base + 17]]);
        // suffix 0x00 = workstation name; flag bit 15 = 0 means unique (not group)
        if suffix == 0x00 && (flags & 0x8000) == 0 {
            let name: String = buf[base..base + 15]
                .iter()
                .take_while(|&&b| b != b' ' && b != 0x00)
                .map(|&b| b as char)
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }

    None
}

// ── Unified Probe Interface ───────────────────────────────────────────────────

pub async fn probe_friendly_name(ip: Ipv4Addr) -> Option<String> {
    // 1. mDNS reverse PTR (macOS, Linux/Avahi)
    if let Some(name) = probe_mdns(ip).await {
        return Some(name);
    }

    // 2. NetBIOS NBNS (Windows)
    if let Some(name) = probe_nbns(ip).await {
        return Some(name);
    }

    // 3. SSDP UPnP XML descriptor (routers, smart TVs, IoT)
    if let Some(name) = probe_ssdp(ip).await {
        return Some(name);
    }

    None
}
