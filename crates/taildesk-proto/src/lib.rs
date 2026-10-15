//! Protocolo de fio do TailDesk.
//!
//! - [`media`]: header binário fixo de 16 bytes dos datagramas de mídia (`07-protocolo.md` §4).
//! - [`probe`]: payload dos datagramas de sondagem de banda (`14-selecao-adaptativa.md` §3.2).
//! - [`net`]: mensagens e limites da seleção adaptativa (`14-selecao-adaptativa.md` §4).
//! - [`files`]: cabeçalho do stream de dados e limites da troca de arquivos (`10-plano` §3).
//! - Mensagens de controle (protobuf/prost) entram na fase 0, junto com o transporte.

pub mod files;
pub mod media;
pub mod net;
pub mod probe;

/// ALPN do QUIC para a versão 1 do protocolo.
pub const ALPN_V1: &[u8] = b"taildesk/1";

/// Porta UDP padrão do endpoint QUIC na tailnet.
pub const DEFAULT_PORT: u16 = 7420;
