//! Protocolo de fio do TailDesk.
//!
//! - [`media`]: header binário fixo de 16 bytes dos datagramas de mídia (`07-protocolo.md` §4).
//! - Mensagens de controle (protobuf/prost) entram na fase 0, junto com o transporte.

pub mod media;

/// ALPN do QUIC para a versão 1 do protocolo.
pub const ALPN_V1: &[u8] = b"taildesk/1";

/// Porta UDP padrão do endpoint QUIC na tailnet.
pub const DEFAULT_PORT: u16 = 7420;
