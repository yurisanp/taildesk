//! `SystemOverlay`: Tailscale instalado na máquina (`tailscaled` + LocalAPI).

use std::io;
use std::net::{IpAddr, SocketAddr};

use taildesk_tsapi::{LocalApi, PeerStatus, PingResult};

use crate::{DatagramSocket, Overlay, PathInfo, PathKind, PeerIdentity, PeerInfo};

/// Overlay sobre o `tailscaled` do sistema.
#[derive(Clone, Debug, Default)]
pub struct SystemOverlay {
    api: LocalApi,
}

impl SystemOverlay {
    pub fn new(api: LocalApi) -> SystemOverlay {
        SystemOverlay { api }
    }

    pub fn api(&self) -> &LocalApi {
        &self.api
    }

    /// Resolve um nome da tailnet (curto, MagicDNS ou hostname) ou um IP para o IP da tailnet.
    pub async fn resolve(&self, name: &str) -> io::Result<IpAddr> {
        if let Ok(ip) = name.parse::<IpAddr>() {
            return Ok(ip);
        }
        let status = self.api.status().await?;
        status
            .peer_by_name(name)
            .and_then(preferred_ip)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{name} não está na sua tailnet"),
                )
            })
    }
}

/// IPv4 da tailnet quando houver (100.x), senão o primeiro.
fn preferred_ip(p: &PeerStatus) -> Option<IpAddr> {
    p.tailscale_ips
        .iter()
        .find(|ip| ip.is_ipv4())
        .or(p.tailscale_ips.first())
        .copied()
}

fn is_local_net(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        // fc00::/7 (ULA) e fe80::/10 (link-local).
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    }
}

fn endpoint_ip(s: &str) -> Option<IpAddr> {
    s.parse::<SocketAddr>().ok().map(|a| a.ip())
}

/// Caminho até o peer a partir do `status` e do `ping` disco do `tailscaled`.
///
/// O `ping` manda: endpoint preenchido = direto (LAN se o endereço for de rede local), região
/// DERP ou peer relay = relay. Sem `ping`, um `CurAddr` preenchido também indica direto; vazio
/// pode ser relay ou só conexão ociosa, então fica `Unknown`.
pub fn path_from(peer: Option<&PeerStatus>, ping: Option<&PingResult>) -> PathInfo {
    let peer_endpoints: Vec<SocketAddr> = peer
        .and_then(|p| p.addrs.as_ref())
        .map(|a| a.iter().filter_map(|s| s.parse().ok()).collect())
        .unwrap_or_default();
    let same_lan_hint = peer_endpoints.iter().any(|a| is_local_net(a.ip()));
    let direct = |ip: Option<IpAddr>| match ip {
        Some(ip) if is_local_net(ip) => PathKind::DirectLan,
        _ => PathKind::DirectWan,
    };

    let (kind, derp_region) = match ping.filter(|p| p.err.is_empty()) {
        Some(p) if !p.endpoint.is_empty() => (direct(endpoint_ip(&p.endpoint)), None),
        Some(p) if !p.derp_region_code.is_empty() => {
            (PathKind::Relayed, Some(p.derp_region_code.clone()))
        }
        Some(p) if !p.peer_relay.is_empty() => (PathKind::Relayed, None),
        _ => match peer.filter(|p| !p.cur_addr.is_empty()) {
            Some(p) => (direct(endpoint_ip(&p.cur_addr)), None),
            None => (PathKind::Unknown, None),
        },
    };
    PathInfo {
        kind,
        rtt: ping.and_then(PingResult::latency),
        derp_region: derp_region.or_else(|| {
            peer.map(|p| p.relay.clone())
                .filter(|r| !r.is_empty() && kind == PathKind::Relayed)
        }),
        same_lan_hint: same_lan_hint || kind == PathKind::DirectLan,
        peer_endpoints,
    }
}

/// Socket UDP do SO preso ao IP da tailnet.
#[derive(Debug)]
pub struct SystemSocket(tokio::net::UdpSocket);

impl SystemSocket {
    pub fn into_inner(self) -> tokio::net::UdpSocket {
        self.0
    }
}

impl DatagramSocket for SystemSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
    }

    async fn send_to(&self, buf: &[u8], to: SocketAddr) -> io::Result<usize> {
        self.0.send_to(buf, to).await
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.0.recv_from(buf).await
    }
}

impl Overlay for SystemOverlay {
    type Socket = SystemSocket;

    async fn local_addr(&self) -> io::Result<IpAddr> {
        let status = self.api.status().await?;
        if status.backend_state != "Running" {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                format!(
                    "Tailscale não está conectado (estado {})",
                    status.backend_state
                ),
            ));
        }
        preferred_ip(&status.self_node)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "sem IP na tailnet"))
    }

    async fn udp_bind(&self, port: u16) -> io::Result<SystemSocket> {
        let ip = self.local_addr().await?;
        tokio::net::UdpSocket::bind((ip, port))
            .await
            .map(SystemSocket)
    }

    async fn peers(&self) -> io::Result<Vec<PeerInfo>> {
        let status = self.api.status().await?;
        Ok(status
            .peers()
            .filter_map(|p| {
                Some(PeerInfo {
                    hostname: p.short_name().to_owned(),
                    addr: preferred_ip(p)?,
                    os: Some(p.os.clone()).filter(|o| !o.is_empty()),
                    online: p.online,
                    tags: p.tags.clone().unwrap_or_default(),
                })
            })
            .collect())
    }

    async fn identify(&self, remote: SocketAddr) -> io::Result<PeerIdentity> {
        let w = self.api.whois(remote).await?;
        if w.node.stable_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{remote} não é um nó da tailnet"),
            ));
        }
        let tags = w.node.tags.unwrap_or_default();
        Ok(PeerIdentity {
            node_stable_id: w.node.stable_id,
            hostname: w.node.name.split('.').next().unwrap_or_default().to_owned(),
            // Nós com tag não pertencem a um usuário; o perfil vem como "tagged-devices".
            user_login: Some(w.user_profile.login_name)
                .filter(|l| !l.is_empty() && tags.is_empty()),
            tags,
            app_caps: Vec::new(),
        })
    }

    async fn path(&self, remote: IpAddr) -> PathInfo {
        let status = self.api.status().await.ok();
        let peer = status.as_ref().and_then(|s| s.peer_by_ip(remote));
        let ping = self.api.ping(remote).await.ok();
        path_from(peer, ping.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn peer(cur: &str, addrs: &[&str]) -> PeerStatus {
        PeerStatus {
            host_name: "desktop-casa".into(),
            tailscale_ips: vec!["100.64.0.2".parse().unwrap()],
            addrs: Some(addrs.iter().map(|s| (*s).to_owned()).collect()),
            cur_addr: cur.into(),
            relay: "sao".into(),
            ..PeerStatus::default()
        }
    }

    fn ping(endpoint: &str, derp: &str) -> PingResult {
        PingResult {
            latency_seconds: 0.002,
            endpoint: endpoint.into(),
            derp_region_code: derp.into(),
            ..PingResult::default()
        }
    }

    #[test]
    fn same_router_ping_is_direct_lan() {
        let p = peer("", &["192.168.0.20:41641", "203.0.113.9:41641"]);
        let info = path_from(Some(&p), Some(&ping("192.168.0.20:41641", "")));
        assert_eq!(info.kind, PathKind::DirectLan);
        assert_eq!(info.rtt, Some(Duration::from_millis(2)));
        assert!(info.same_lan_hint);
        assert_eq!(info.peer_endpoints.len(), 2);
        assert_eq!(info.derp_region, None);
    }

    #[test]
    fn public_endpoint_is_direct_wan_and_derp_is_relayed() {
        let p = peer("", &["203.0.113.9:41641"]);
        assert_eq!(
            path_from(Some(&p), Some(&ping("203.0.113.9:41641", ""))).kind,
            PathKind::DirectWan
        );
        let relayed = path_from(Some(&p), Some(&ping("", "gru")));
        assert_eq!(relayed.kind, PathKind::Relayed);
        assert_eq!(relayed.derp_region.as_deref(), Some("gru"));
        assert!(!relayed.same_lan_hint);
    }

    #[test]
    fn without_ping_uses_cur_addr_or_stays_unknown() {
        assert_eq!(
            path_from(Some(&peer("[fe80::1]:41641", &[])), None).kind,
            PathKind::DirectLan
        );
        assert_eq!(
            path_from(Some(&peer("", &[])), None).kind,
            PathKind::Unknown
        );
        let failed = PingResult {
            err: "timeout".into(),
            ..ping("192.168.0.20:41641", "")
        };
        assert_eq!(path_from(None, Some(&failed)).kind, PathKind::Unknown);
        assert_eq!(path_from(None, None), PathInfo::default());
    }

    #[test]
    fn local_net_ranges() {
        for ip in [
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.1",
            "169.254.1.1",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(is_local_net(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["100.64.0.1", "8.8.8.8", "2001:db8::1"] {
            assert!(!is_local_net(ip.parse().unwrap()), "{ip}");
        }
    }
}
