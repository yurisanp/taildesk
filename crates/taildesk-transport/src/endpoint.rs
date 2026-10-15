//! Endpoints QUIC sobre um socket UDP do SO.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};

use crate::tls::{self, Identity, Pin};

/// Buffers de datagrama grandes o bastante para um degrau de 100 ms a 200 Mbit/s (2,5 MB).
const DATAGRAM_BUFFER: usize = 8 * 1024 * 1024;

fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(5)))
        .datagram_receive_buffer_size(Some(DATAGRAM_BUFFER))
        .datagram_send_buffer_size(DATAGRAM_BUFFER);
    Arc::new(t)
}

fn other(e: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::other(e)
}

/// Endpoint que aceita conexões em `addr` com a identidade dada.
pub fn server_endpoint(addr: SocketAddr, identity: &Identity) -> io::Result<quinn::Endpoint> {
    let crypto =
        QuicServerConfig::try_from(identity.server_config().map_err(other)?).map_err(other)?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    cfg.transport_config(transport_config());
    quinn::Endpoint::server(cfg, addr)
}

/// Endpoint só de saída, numa porta efêmera da família de `remote`.
pub fn client_endpoint(remote: SocketAddr) -> io::Result<quinn::Endpoint> {
    let bind: SocketAddr = if remote.is_ipv4() {
        (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
    };
    quinn::Endpoint::client(bind)
}

/// Conecta a `remote` conferindo o certificado com `pin`.
pub async fn connect(
    endpoint: &quinn::Endpoint,
    remote: SocketAddr,
    pin: Pin,
) -> io::Result<quinn::Connection> {
    let crypto =
        QuicClientConfig::try_from(tls::client_config(pin).map_err(other)?).map_err(other)?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(transport_config());
    endpoint
        .connect_with(cfg, remote, tls::SERVER_NAME)
        .map_err(other)?
        .await
        .map_err(other)
}
