//! Camada de rede da tailnet.
//!
//! Todo acesso à rede passa pela trait [`Overlay`] (`docs/context/01-tailscale.md` §3 e
//! `10-plano-windows-linux.md` §2.1). O TailDesk usa só o Tailscale instalado na máquina
//! (ADR-009), então há dois backends:
//! - `SystemOverlay`: `tailscaled` ≥ 1.102 + LocalAPI (`taildesk-tsapi`), dá caminho, usuário
//!   e app caps. É o único em produção.
//! - `LanOverlay`: sockets do SO, só para desenvolvimento e testes.
//!
//! A identidade do peer vem sempre de [`Overlay::identify`], nunca do conteúdo das mensagens.

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// Um peer visível na tailnet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerInfo {
    pub hostname: String,
    pub addr: IpAddr,
    pub os: Option<String>,
    pub online: bool,
    pub tags: Vec<String>,
}

/// Quem está do outro lado de uma conexão, segundo a tailnet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerIdentity {
    pub node_stable_id: String,
    pub hostname: String,
    /// Login do usuário, do `whois` da LocalAPI. `None` em nó com tag.
    pub user_login: Option<String>,
    pub tags: Vec<String>,
    /// Conteúdo JSON bruto da app capability do TailDesk, quando disponível.
    pub app_caps: Vec<String>,
}

pub use taildesk_proto::net::PathKind;

/// Caminho de rede até um peer (`14-selecao-adaptativa.md` §1.1). É pista para a seleção
/// adaptativa: a classe de rede sai sempre da medição.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathInfo {
    /// Vem do `status` da LocalAPI (`CurAddr` direto ou `Relay`).
    pub kind: PathKind,
    /// RTT informado pelo overlay (ping do `tailscaled`), se houver.
    pub rtt: Option<Duration>,
    /// Região DERP do peer ("sao", "mia").
    pub derp_region: Option<String>,
    /// Ver [`same_lan_hint`].
    pub same_lan_hint: bool,
    /// Endereços de underlay do peer (os "Endpoints" do Tailscale).
    pub peer_endpoints: Vec<SocketAddr>,
}

/// Pista de "mesmo roteador": algum endpoint IPv4 privado do peer está numa sub-rede de uma
/// interface local, ou o IP público do peer é o nosso (mesmo NAT).
///
/// `local_nets` são os endereços das interfaces locais com o tamanho do prefixo.
pub fn same_lan_hint(
    local_nets: &[(IpAddr, u8)],
    local_public: Option<IpAddr>,
    peer_endpoints: &[SocketAddr],
) -> bool {
    peer_endpoints.iter().any(|ep| {
        let ip = ep.ip();
        if local_public.is_some_and(|p| p == ip) {
            return true;
        }
        let IpAddr::V4(peer) = ip else { return false };
        if !peer.is_private() {
            return false;
        }
        local_nets.iter().any(|&(addr, prefix)| match addr {
            IpAddr::V4(local) if prefix <= 32 => {
                let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
                u32::from(local) & mask == u32::from(peer) & mask && local != peer
            }
            _ => false,
        })
    })
}

/// Socket de datagramas na tailnet, base do endpoint QUIC.
pub trait DatagramSocket: Send + Sync {
    fn local_addr(&self) -> io::Result<SocketAddr>;
    fn send_to(&self, buf: &[u8], to: SocketAddr)
    -> impl Future<Output = io::Result<usize>> + Send;
    fn recv_from(
        &self,
        buf: &mut [u8],
    ) -> impl Future<Output = io::Result<(usize, SocketAddr)>> + Send;
}

/// Backend de rede da tailnet.
pub trait Overlay: Send + Sync + 'static {
    type Socket: DatagramSocket;

    fn local_addr(&self) -> impl Future<Output = io::Result<IpAddr>> + Send;
    fn udp_bind(&self, port: u16) -> impl Future<Output = io::Result<Self::Socket>> + Send;
    fn peers(&self) -> impl Future<Output = io::Result<Vec<PeerInfo>>> + Send;
    fn identify(&self, remote: SocketAddr)
    -> impl Future<Output = io::Result<PeerIdentity>> + Send;
    fn path(&self, remote: IpAddr) -> impl Future<Output = PathInfo> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ep(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn same_subnet_private_endpoint_is_lan() {
        let local = [(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 10)), 24)];
        assert!(same_lan_hint(&local, None, &[ep("192.168.0.20:41641")]));
        assert!(!same_lan_hint(&local, None, &[ep("192.168.1.20:41641")]));
        // Nosso próprio endereço não conta.
        assert!(!same_lan_hint(&local, None, &[ep("192.168.0.10:41641")]));
    }

    #[test]
    fn same_public_ip_is_lan_and_public_subnets_are_not() {
        let public: IpAddr = "203.0.113.7".parse().unwrap();
        assert!(same_lan_hint(&[], Some(public), &[ep("203.0.113.7:3478")]));
        let local = [(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)), 24)];
        assert!(!same_lan_hint(&local, None, &[ep("203.0.113.8:3478")]));
        assert!(!same_lan_hint(&local, None, &[]));
    }

    #[test]
    fn odd_prefixes_do_not_panic() {
        let peer = [ep("10.1.2.3:1")];
        let any = [(IpAddr::V4(Ipv4Addr::new(10, 9, 9, 9)), 0)];
        assert!(same_lan_hint(&any, None, &peer));
        let bad = [(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 4)), 40)];
        assert!(!same_lan_hint(&bad, None, &peer));
    }
}
