//! Sessão: autorização no host, consentimento e as etapas de conexão dos dois lados
//! (`06-arquitetura.md` §6–7, `12-levantamento-ux.md` §5.3–5.5).
//!
//! - [`policy`]: quem pode pedir acesso e o que oferecer (configuração "Este computador").
//! - [`host`]: `Hello` -> diálogo de consentimento -> ao vivo; filtro de permissões por mensagem.
//! - [`controller`]: "Conectando" -> "Esperando aceitar" -> ao vivo, ou o motivo do fim.
//! - [`cursor`]: cursor separado (posição e forma à parte do vídeo).
//!
//! Lógica pura, sem rede nem UI: quem usa liga as mensagens do stream de controle e o relógio.

pub mod controller;
pub mod cursor;
pub mod host;
pub mod policy;

pub use controller::{ControllerSession, ControllerState, EndReason};
pub use cursor::{AcaoForma, CursorControlador, CursorHost, Desenho, Forma};
pub use host::{
    AckFields, CONSENT_TIMEOUT, ConsentAnswer, ConsentPrompt, HelloReply, HostSession, HostState,
    Need, need,
};
pub use policy::{Decision, HostSettings, TrustedDevice};
