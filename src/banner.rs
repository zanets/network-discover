use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_millis(800);

pub async fn grab(stream: TcpStream, port: u16, ip: Ipv4Addr) -> Option<String> {
    tokio::time::timeout(TIMEOUT, probe(stream, port, ip))
        .await
        .ok()
        .flatten()
}

async fn probe(mut s: TcpStream, port: u16, ip: Ipv4Addr) -> Option<String> {
    let raw = match port {
        // Protocols that emit a text banner immediately on connect
        21 | 22 | 25 | 110 | 119 | 143 | 194 | 465 | 514 | 587 | 993 | 995 => {
            read_line(&mut s).await?
        }
        // HTTP — request headers then parse Server:
        80 | 3000 | 5000 | 8000 | 8008 | 8080 | 8081 | 8082 | 8083 | 8088 | 8089
        | 8090 | 8091 | 8888 | 9000 | 9090 | 10000 => http_banner(&mut s, ip).await?,
        // Redis — send PING
        6379 => {
            s.write_all(b"PING\r\n").await.ok()?;
            let line = read_line(&mut s).await?;
            line.trim_start_matches('+').trim().to_string()
        }
        // Everything else: passive read of any greeting bytes
        _ => read_greeting(&mut s).await?,
    };

    let clean = raw.trim().to_string();
    if clean.is_empty() {
        None
    } else {
        Some(clean.chars().take(72).collect())
    }
}

async fn read_line(s: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 512];
    let n = s.read(&mut buf).await.ok().filter(|&n| n > 0)?;
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .map(|l| l.trim().to_string())
}

async fn http_banner(s: &mut TcpStream, ip: Ipv4Addr) -> Option<String> {
    let req = format!(
        "HEAD / HTTP/1.0\r\nHost: {ip}\r\nUser-Agent: nd/0.1\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.ok()?;

    let mut buf = vec![0u8; 2048];
    let n = s.read(&mut buf).await.ok().filter(|&n| n > 0)?;
    let text = String::from_utf8_lossy(&buf[..n]);

    for line in text.lines() {
        if line.to_lowercase().starts_with("server:") {
            let val = line[7..].trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    // Fall back to status line
    text.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

async fn read_greeting(s: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 256];
    let n = tokio::time::timeout(Duration::from_millis(300), s.read(&mut buf))
        .await
        .ok()?
        .ok()
        .filter(|&n| n > 0)?;

    let clean: String = buf[..n]
        .iter()
        .filter_map(|&b| match b {
            0x20..=0x7E => Some(b as char),
            b'\n' | b'\r' | b'\t' => Some(' '),
            _ => None,
        })
        .collect();

    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.len() < 3 { None } else { Some(clean) }
}
