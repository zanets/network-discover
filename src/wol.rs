use anyhow::Result;
use tokio::net::UdpSocket;

/// Send an IEEE 802.3 Wake-on-LAN magic packet to the broadcast address.
pub async fn send(mac: [u8; 6]) -> Result<()> {
    // Magic packet: 6×0xFF followed by the target MAC repeated 16 times
    let mut packet = [0u8; 102];
    packet[..6].fill(0xFF);
    for i in 0..16 {
        packet[6 + i * 6..12 + i * 6].copy_from_slice(&mac);
    }

    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.set_broadcast(true)?;
    socket.send_to(&packet, "255.255.255.255:9").await?;
    Ok(())
}
