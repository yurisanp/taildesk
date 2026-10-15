//! Camada de rede da tailnet.
//!
//! Todo acesso à rede passa pela trait [`Overlay`], com três backends previstos
//! (`docs/context/01-tailscale.md` §3 e `10-plano-windows-linux.md` §2.1):
//! - `EmbeddedOverlay`: crate `tailscale` (tailscale-rs) `=0.6.1`, padrão no Windows x64 e Linux.
//! - `SystemOverlay`: `tailscaled` do sistema + LocalAPI (`taildesk-tsapi`), dá usuário e app caps.
//! - `LanOverlay`: sockets do SO, só para desenvolvimento.
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
    /// Login do usuário. `None` no modo embutido (tailscale-rs ainda não expõe whois).
    pub user_login: Option<String>,
    pub tags: Vec<String>,
    /// Conteúdo JSON bruto da app capability do TailDesk, quando disponível.
    pub app_caps: Vec<String>,
}

/// Caminho de rede até um peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathKind {
    Direct { rtt: Option<Duration> },
    Relayed { derp_region: Option<String> },
    Unknown,
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
    fn path(&self, remote: IpAddr) -> impl Future<Output = PathKind> + Send;
}
