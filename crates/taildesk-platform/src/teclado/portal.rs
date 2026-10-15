//! Injeção de teclas no Linux pelo portal RemoteDesktop (`11-teclado.md` §2.4), na mesma
//! sessão da captura ([`SessaoPortal`]). Vale para Wayland e para X11 com portal.
//!
//! Teclas vão pelo código evdev (posição física; o layout do host decide o caractere, como a
//! busca reversa de `taildesk_keyboard::Host` espera). Caracteres fora do layout vão pelo keysym
//! Unicode do X e o compositor acha ou reserva uma tecla para ele.

use taildesk_keyboard::Inject;
use taildesk_keyboard::codes::hid_to_evdev;

use crate::wayland::{ErroPortal, SessaoPortal};

/// Keysym do X para um caractere: Latin-1 imprimível é o próprio código, o resto é
/// `0x0100_0000 + U+XXXX`. Caracteres de controle não têm keysym de texto.
pub fn keysym_de(c: char) -> Option<i32> {
    let cp = u32::from(c);
    match cp {
        0x20..=0x7E | 0xA0..=0xFF => Some(cp as i32),
        0x100..=0x10_FFFF if !c.is_control() => Some((0x0100_0000 | cp) as i32),
        _ => None,
    }
}

/// Injetor do portal. O portal é assíncrono (D-Bus), então não implementa
/// [`super::InjetorTeclado`]: rode na tarefa de input da sessão, nunca na thread de mídia.
#[derive(Debug, Clone, Copy)]
pub struct InjetorPortal<'a> {
    sessao: &'a SessaoPortal,
}

impl<'a> InjetorPortal<'a> {
    pub fn new(sessao: &'a SessaoPortal) -> Self {
        InjetorPortal { sessao }
    }

    /// Injeta na ordem. Teclas sem código evdev e caracteres sem keysym são pulados.
    pub async fn injetar(&self, eventos: &[Inject]) -> Result<(), ErroPortal> {
        for ev in eventos {
            match *ev {
                Inject::Key { hid, down } => {
                    if let Some(evdev) = hid_to_evdev(hid) {
                        self.sessao.tecla_keycode(i32::from(evdev), down).await?;
                    }
                }
                Inject::Unicode(c) => {
                    if let Some(keysym) = keysym_de(c) {
                        self.sessao.tecla_keysym(keysym, true).await?;
                        self.sessao.tecla_keysym(keysym, false).await?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms() {
        assert_eq!(keysym_de('a'), Some(0x61));
        assert_eq!(keysym_de('ç'), Some(0xE7));
        assert_eq!(keysym_de('\u{A0}'), Some(0xA0));
        assert_eq!(keysym_de('ć'), Some(0x0100_0107));
        assert_eq!(keysym_de('€'), Some(0x0100_20AC));
        assert_eq!(keysym_de('😀'), Some(0x0101_F600));
        assert_eq!(keysym_de('\n'), None);
        assert_eq!(keysym_de('\u{7F}'), None);
        assert_eq!(keysym_de('\u{85}'), None);
    }
}
