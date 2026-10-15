//! Interface do app TailDesk em Slint. Reaproveita os tokens e componentes do protótipo
//! (`apps/taildesk-prototipo`, levantamento de UX em `docs/design/12-levantamento-ux.md`), agora
//! com dados reais: o app preenche as propriedades e trata os callbacks.
//!
//! Slint entra pela Royalty-free License; a atribuição fica em Configurações → Sobre.

#![allow(missing_debug_implementations, clippy::all)]

slint::include_modules!();
