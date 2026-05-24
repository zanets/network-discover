use std::net::Ipv4Addr;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};

pub struct PortResult {
    pub port: u16,
    pub service: &'static str,
    pub banner: Option<String>,
}

pub enum PortScanEvent {
    Open(PortResult),
    Banner { port: u16, banner: String },
    Progress { done: usize, total: usize },
    Done,
}

pub const PORTS: &[u16] = &[
    // Well-known
    21, 22, 23, 25, 53, 67, 69, 80, 88, 110, 111, 119, 123, 135, 137, 138, 139, 143,
    161, 162, 179, 389, 443, 445, 465, 514, 515, 587, 631, 636, 873, 902, 990, 993, 995,
    // Common registered ports
    1080, 1194, 1433, 1521, 1723, 1883, 2049, 2181, 2375, 2376, 3000, 3306, 3389,
    3690, 4369, 4444, 5000, 5001, 5432, 5672, 5900, 5984, 6379, 6443, 7474,
    8000, 8008, 8080, 8081, 8082, 8083, 8088, 8089, 8090, 8091, 8443, 8444,
    8888, 8983, 9000, 9090, 9092, 9200, 9300, 9418, 9443, 9999, 10000,
    11211, 15672, 27017, 27018, 50000,
];

pub fn service_name(port: u16) -> &'static str {
    match port {
        21 => "FTP",
        22 => "SSH",
        23 => "Telnet",
        25 => "SMTP",
        53 => "DNS",
        67 | 68 => "DHCP",
        69 => "TFTP",
        80 => "HTTP",
        88 => "Kerberos",
        110 => "POP3",
        111 => "RPC",
        119 => "NNTP",
        123 => "NTP",
        135 => "MSRPC",
        137 | 138 | 139 => "NetBIOS",
        143 => "IMAP",
        161 | 162 => "SNMP",
        179 => "BGP",
        389 => "LDAP",
        443 => "HTTPS",
        445 => "SMB",
        465 => "SMTPS",
        514 => "Syslog",
        515 => "LPD",
        587 => "SMTP/TLS",
        631 => "IPP",
        636 => "LDAPS",
        873 => "rsync",
        902 => "VMware",
        990 => "FTPS",
        993 => "IMAPS",
        995 => "POP3S",
        1080 => "SOCKS",
        1194 => "OpenVPN",
        1433 => "MSSQL",
        1521 => "Oracle",
        1723 => "PPTP",
        1883 => "MQTT",
        2049 => "NFS",
        2181 => "Zookeeper",
        2375 | 2376 => "Docker",
        3000 => "HTTP-alt",
        3306 => "MySQL",
        3389 => "RDP",
        3690 => "SVN",
        4369 => "Erlang",
        4444 => "Shell",
        5000 | 5001 => "HTTP-alt",
        5432 => "PostgreSQL",
        5672 => "AMQP",
        5900 => "VNC",
        5984 => "CouchDB",
        6379 => "Redis",
        6443 => "k8s-API",
        7474 => "Neo4j",
        8000 | 8001 => "HTTP-alt",
        8008 => "HTTP-alt",
        8080 | 8081 | 8082 | 8083 => "HTTP-alt",
        8088 | 8089 | 8090 | 8091 => "HTTP-alt",
        8443 | 8444 => "HTTPS-alt",
        8888 => "Jupyter/HTTP",
        8983 => "Solr",
        9000 => "HTTP-alt",
        9090 => "Prometheus",
        9092 => "Kafka",
        9200 | 9300 => "Elasticsearch",
        9418 => "Git",
        9443 => "HTTPS-alt",
        9999 => "HTTP-alt",
        10000 => "Webmin",
        11211 => "Memcached",
        15672 => "RabbitMQ",
        27017 | 27018 => "MongoDB",
        50000 => "DB2",
        _ => "",
    }
}

pub async fn scan(ip: Ipv4Addr, tx: mpsc::Sender<PortScanEvent>) {
    let total = PORTS.len();
    let sem = Arc::new(Semaphore::new(100));
    let done = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::with_capacity(total);

    for &port in PORTS {
        let sem = sem.clone();
        let tx = tx.clone();
        let done = done.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let conn = tokio::time::timeout(
                Duration::from_millis(500),
                tokio::net::TcpStream::connect((ip, port)),
            )
            .await;

            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            tx.send(PortScanEvent::Progress { done: n, total }).await.ok();

            if let Ok(Ok(stream)) = conn {
                tx.send(PortScanEvent::Open(PortResult {
                    port,
                    service: service_name(port),
                    banner: None,
                }))
                .await
                .ok();

                // Grab banner asynchronously — port appears immediately, banner fills in
                let tx2 = tx.clone();
                tokio::spawn(async move {
                    if let Some(b) = crate::banner::grab(stream, port, ip).await {
                        tx2.send(PortScanEvent::Banner { port, banner: b }).await.ok();
                    }
                });
            }
        }));
    }

    for h in handles {
        h.await.ok();
    }
    tx.send(PortScanEvent::Done).await.ok();
    // Banner tasks may still be running; channel stays open until all tx clones drop
}
