//! Protocolo de fio do TailDesk.
//!
//! - [`control`]: mensagens do stream de controle em protobuf (`07-protocolo.md` §2–3), com vídeo
//!   por monitor (`13-multimonitor.md` §3) e áudio reservado (`15-audio.md` §6.5).
//! - [`media`]: header binário fixo de 16 bytes dos datagramas de mídia (`07-protocolo.md` §4).
//! - [`probe`]: payload dos datagramas de sondagem de banda (`14-selecao-adaptativa.md` §3.2).
//! - [`net`]: mensagens e limites da seleção adaptativa (`14-selecao-adaptativa.md` §4).

pub mod control;
pub mod media;
pub mod net;
pub mod probe;

/// ALPN do QUIC para a versão 1 do protocolo.
pub const ALPN_V1: &[u8] = b"taildesk/1";

/// Porta UDP padrão do endpoint QUIC na tailnet.
pub const DEFAULT_PORT: u16 = 7420;
