//! Wake-on-LAN magic packet sender. Runs on the control machine, which must
//! be L2-local to the box (same LAN) for broadcasts to reach it.

use anyhow::{Context, Result};
use std::net::UdpSocket;

#[derive(serde::Deserialize, Clone)]
pub struct WolTarget {
    pub mac: String,
    #[serde(default)]
    pub ip: Option<String>,
}

/// Send the magic packet: broadcast on the local network, plus a best-effort
/// unicast to the box's LAN IP if known.
pub fn send_wol(target: &WolTarget) -> Result<()> {
    let packet = magic_packet(&target.mac).context("parsing MAC address")?;
    let sock = UdpSocket::bind("0.0.0.0:0").context("binding UDP socket")?;
    sock.set_broadcast(true).context("enabling broadcast")?;
    sock.send_to(&packet, "255.255.255.255:9")
        .context("broadcast to 255.255.255.255:9")?;
    let _ = sock.send_to(&packet, "0.0.0.0:9");
    if let Some(ip) = &target.ip {
        let _ = sock.send_to(&packet, (ip.as_str(), 9));
    }
    Ok(())
}

fn magic_packet(mac: &str) -> Result<[u8; 102]> {
    let octets: Vec<u8> = mac
        .split([':', '-'])
        .map(|o| u8::from_str_radix(o.trim(), 16).with_context(|| format!("bad MAC octet {o:?} in {mac:?}")))
        .collect::<Result<_>>()?;
    anyhow::ensure!(octets.len() == 6, "MAC must have 6 octets, got {}", octets.len());
    let mut packet = [0u8; 102];
    packet[..6].copy_from_slice(&[0xffu8; 6]);
    for chunk in packet[6..].chunks_exact_mut(6) {
        chunk.copy_from_slice(&octets);
    }
    Ok(packet)
}
