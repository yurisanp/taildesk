//! Teclado no SO: leitura do layout local para a tradução de `taildesk-keyboard` e injeção das
//! teclas que o `taildesk_keyboard::Host` decide (`11-teclado.md` §4).

use std::io;

use taildesk_keyboard::Inject;

/// Injeta no SO local uma sequência de teclas já traduzida para o layout do host.
pub trait InjetorTeclado {
    fn injetar(&mut self, eventos: &[Inject]) -> io::Result<()>;
}

#[cfg(target_os = "linux")]
pub mod portal;
#[cfg(target_os = "linux")]
pub mod xkb;

#[cfg(windows)]
pub mod sendinput;
#[cfg(windows)]
pub mod windows;
