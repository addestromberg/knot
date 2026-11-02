//! Opt-in LAN announcements and an optional external WireGuard mesh adapter.
use crate::{Error, RemoteAddress, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, UdpSocket},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const PEER_PORT: u16 = 47568;
pub const PEER_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 75, 78);
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerAnnouncement {
    pub protocol: String,
    pub name: String,
    pub url: String,
    pub expires: u64,
}
fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::Invalid(e.to_string()))?
        .as_secs())
}
impl PeerAnnouncement {
    pub fn decode(bytes: &[u8], now: u64) -> Result<Self> {
        if bytes.len() > 4096 {
            return Err(Error::Invalid("peer announcement too large".into()));
        }
        let announcement: Self = serde_json::from_slice(bytes)?;
        if announcement.protocol != "knot-peer-v1"
            || announcement.name.trim().is_empty()
            || announcement.name.len() > 128
            || announcement.name.chars().any(char::is_control)
            || announcement.expires < now
            || announcement.expires > now.saturating_add(120)
            || !matches!(
                RemoteAddress::parse(&announcement.url)?,
                RemoteAddress::Ssh { .. } | RemoteAddress::Mesh { .. }
            )
        {
            return Err(Error::Invalid(
                "invalid or expired peer announcement".into(),
            ));
        }
        Ok(announcement)
    }
}
pub fn announce_peer(name: &str, url: &str, duration: Duration) -> Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_multicast_ttl_v4(1)?;
    let started = Instant::now();
    loop {
        let announcement = PeerAnnouncement {
            protocol: "knot-peer-v1".into(),
            name: name.into(),
            url: url.into(),
            expires: now()?.saturating_add(15),
        };
        let bytes = serde_json::to_vec(&announcement)?;
        PeerAnnouncement::decode(&bytes, now()?)?;
        socket.send_to(&bytes, (PEER_GROUP, PEER_PORT))?;
        if !duration.is_zero() && started.elapsed() >= duration {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2).min(if duration.is_zero() {
            Duration::from_secs(2)
        } else {
            duration.saturating_sub(started.elapsed())
        }));
    }
}
pub fn discover_peers(duration: Duration) -> Result<Vec<PeerAnnouncement>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PEER_PORT))?;
    socket.join_multicast_v4(&PEER_GROUP, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    let started = Instant::now();
    let mut peers = BTreeMap::new();
    let mut buffer = [0u8; 4097];
    while started.elapsed() < duration {
        match socket.recv_from(&mut buffer) {
            Ok((len, _)) => {
                if let Ok(announcement) = PeerAnnouncement::decode(&buffer[..len], now()?) {
                    peers.insert(announcement.url.clone(), announcement);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(peers
        .into_values()
        .filter(|peer| peer.expires >= now().unwrap_or(u64::MAX))
        .collect())
}

#[derive(Clone, Debug, Serialize)]
pub struct MeshPeer {
    pub hostname: String,
    pub dns_name: String,
    pub addresses: Vec<String>,
    pub online: bool,
}
/// Preserve the provider's typed address list; no automatic trust or remote creation.
pub fn mesh_peers_from_json(value: &serde_json::Value) -> Result<Vec<MeshPeer>> {
    let backend = value["BackendState"].as_str().unwrap_or("Unknown");
    if backend != "Running" {
        return Err(Error::Conflict(format!(
            "Tailscale is {backend}; run knot peer mesh-up"
        )));
    }
    let mut peers = Vec::new();
    for entry in value["Peer"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.values())
    {
        let addresses: Vec<String> = serde_json::from_value(entry["TailscaleIPs"].clone())?;
        if addresses
            .iter()
            .any(|address| address.parse::<std::net::IpAddr>().is_err())
        {
            return Err(Error::Integrity("invalid mesh peer IP address".into()));
        }
        peers.push(MeshPeer {
            hostname: entry["HostName"].as_str().unwrap_or("").into(),
            dns_name: entry["DNSName"]
                .as_str()
                .unwrap_or("")
                .trim_end_matches('.')
                .into(),
            addresses,
            online: entry["Online"].as_bool().unwrap_or(false),
        });
    }
    peers.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    Ok(peers)
}
pub fn mesh_peers() -> Result<Vec<MeshPeer>> {
    let output = Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound(
                    "Tailscale CLI is not installed; install the optional provider first".into(),
                )
            } else {
                error.into()
            }
        })?;
    if !output.status.success() {
        return Err(Error::Conflict(format!(
            "Tailscale status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    mesh_peers_from_json(&serde_json::from_slice(&output.stdout)?)
}
pub fn mesh_up(ssh: bool, login_server: Option<&str>) -> Result<()> {
    let mut command = Command::new("tailscale");
    command.arg("up");
    if ssh {
        command.arg("--ssh");
    }
    if let Some(server) = login_server {
        if !server.starts_with("https://") || server.chars().any(char::is_control) {
            return Err(Error::Invalid(
                "mesh login server must be an HTTPS URL".into(),
            ));
        }
        command.arg("--login-server").arg(server);
    }
    // Authentication URLs/prompts are diagnostics; retain clean JSON stdout for Knot.
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound("Tailscale CLI is not installed".into())
            } else {
                error.into()
            }
        })?;
    if let Some(mut output) = child.stdout.take() {
        std::io::copy(&mut output, &mut std::io::stderr())?;
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(Error::Conflict(
            "Tailscale setup did not complete; follow provider authentication/daemon instructions"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_is_only_a_bounded_untrusted_connection_hint() {
        let announcement = PeerAnnouncement {
            protocol: "knot-peer-v1".into(),
            name: "workshop".into(),
            url: "ssh://engineer@workshop/home/engineer/project".into(),
            expires: 110,
        };
        let bytes = serde_json::to_vec(&announcement).unwrap_or_default();
        assert!(PeerAnnouncement::decode(&bytes, 100).is_ok());
        assert!(PeerAnnouncement::decode(&bytes, 200).is_err());
        assert!(PeerAnnouncement::decode(&bytes, 0).is_ok());
        assert!(PeerAnnouncement::decode(&bytes, 1000).is_err());
        assert!(PeerAnnouncement::decode(&vec![b'x'; 4097], 100).is_err());
        assert!(PeerAnnouncement::decode(br#"{"protocol":"knot-peer-v1","name":"peer","url":"file:///secret","expires":110}"#,100).is_err());
    }
    #[test]
    fn mesh_adapter_validates_addresses_and_reports_authentication_state() {
        let value = serde_json::json!({"BackendState":"Running","Peer":{"node":{"HostName":"workshop","DNSName":"workshop.example.ts.net.","TailscaleIPs":["100.64.0.2","fd7a:115c:a1e0::2"],"Online":true}}});
        let peers = mesh_peers_from_json(&value).unwrap_or_default();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].dns_name, "workshop.example.ts.net");
        assert!(peers[0].online);
        assert!(mesh_peers_from_json(&serde_json::json!({"BackendState":"NeedsLogin"})).is_err());
    }
}
