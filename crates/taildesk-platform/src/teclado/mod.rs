//! Teclado no SO: leitura do layout local para a tradução de `taildesk-keyboard`
//! (`11-teclado.md` §4). A injeção entra junto com a sessão de controle.

#[cfg(target_os = "linux")]
pub mod xkb;

#[cfg(windows)]
pub mod windows;
