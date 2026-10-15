//! QUIC (`quinn`) do TailDesk: stream bidi de controle, streams uni para arquivos e datagramas
//! para mídia. Ver `docs/context/07-protocolo.md`.
//!
//! Primeira fatia (fase 0), feita para ligar a sondagem de banda (`14-selecao-adaptativa.md`):
//! - [`tls`]: identidade autoassinada e verificação por *fingerprint* fixado (TOFU);
//! - [`endpoint`]: endpoints servidor/cliente sobre um socket UDP do SO;
//! - [`probe`]: rajada de sondagem e recepção pelos datagramas;
//! - [`bench`]: o teste do `taildesk-cli bench-net` (RTT + banda nos dois sentidos).
//!
//! O TailDesk usa só o Tailscale instalado na máquina (ADR-009): o IP da tailnet é uma interface comum do
//! SO, então o endpoint usa um socket UDP do SO ligado a esse IP (`SystemOverlay::local_addr`).

pub mod bench;
pub mod endpoint;
pub mod probe;
pub mod tls;
pub mod video;

pub use endpoint::{client_endpoint, connect, server_endpoint};
pub use tls::{Identity, Pin, peer_fingerprint};
pub use video::SaidaQuic;
