//! Sessão: autorização no host, consentimento e as etapas de conexão dos dois lados
//! (`06-arquitetura.md` §6–7, `12-levantamento-ux.md` §5.3–5.5).
//!
//! - [`policy`]: quem pode pedir acesso e o que oferecer (configuração "Este computador").
//! - [`host`]: `Hello` -> diálogo de consentimento -> ao vivo; filtro de permissões por mensagem.
//! - [`controller`]: "Conectando" -> "Esperando aceitar" -> ao vivo, ou o motivo do fim.
//! - [`monitores`]: monitores um por vez (abas, atalhos, mouse por monitor).
//! - [`cursor`]: cursor separado (posição e forma à parte do vídeo).
//! - [`clipboard`]: área de transferência compartilhada (oferta, pedido e pedaços).
//! - [`conexao`]: tudo isso sobre QUIC (stream de controle, streams uni, arquivos).
//!
//! Fora de [`conexao`], lógica pura sem rede nem UI. Nenhum módulo tem código de SO: injeção,
//! captura e keyring entram pelas traits de [`conexao::host`].

pub mod clipboard;
pub mod conexao;
pub mod controller;
pub mod cursor;
pub mod host;
pub mod monitores;
pub mod policy;
pub mod senha;

pub use controller::{ControllerSession, ControllerState, EndReason};
pub use cursor::{AcaoForma, CursorControlador, CursorHost, Desenho, Forma};
pub use host::{
    AckFields, CONSENT_TIMEOUT, ConsentAnswer, ConsentPrompt, HelloReply, HostSession, HostState,
    Need, need,
};
pub use monitores::{Aba, Atalho, UmPorVez, atalho};
pub use policy::{Decision, HostSettings, TrustedDevice};
