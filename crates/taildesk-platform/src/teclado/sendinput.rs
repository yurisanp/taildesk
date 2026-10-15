//! Injeção de teclas no Windows com `SendInput` (`11-teclado.md` §2.4).
//!
//! Teclas vão por scancode (`KEYEVENTF_SCANCODE`), que é a posição física: o layout do host
//! decide o caractere, como a busca reversa de `taildesk_keyboard::Host` espera. Caracteres
//! fora do layout vão como `KEYEVENTF_UNICODE`.

use std::io;

use taildesk_keyboard::codes::hid_to_scancode;
use taildesk_keyboard::{Inject, Locks};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, SendInput, VK_CAPITAL, VK_NUMLOCK,
};

use super::InjetorTeclado;

/// Caps Lock e Num Lock ligados agora (para o host sincronizar no modo Físico).
pub fn travas() -> Locks {
    // SAFETY: sem pré-condições; o bit 0 é o estado de alternância.
    let ligado = |vk| unsafe { GetKeyState(i32::from(vk)) } & 1 != 0;
    Locks {
        caps: ligado(VK_CAPITAL),
        num: ligado(VK_NUMLOCK),
    }
}

/// Injetor do Windows. Só funciona na sessão interativa do usuário (fase 5 trata o serviço).
#[derive(Default)]
pub struct InjetorWindows {
    buf: Vec<INPUT>,
}

impl std::fmt::Debug for InjetorWindows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InjetorWindows").finish_non_exhaustive()
    }
}

fn tecla(scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Monta os `INPUT` de uma sequência. Teclas sem scancode conhecido são puladas.
fn entradas(eventos: &[Inject], out: &mut Vec<INPUT>) {
    for ev in eventos {
        match *ev {
            Inject::Key { hid, down } => {
                let Some(scan) = hid_to_scancode(hid) else {
                    continue;
                };
                let mut flags = KEYEVENTF_SCANCODE;
                if scan & 0xFF00 == 0xE000 {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                if !down {
                    flags |= KEYEVENTF_KEYUP;
                }
                out.push(tecla(scan & 0xFF, flags));
            }
            Inject::Unicode(c) => {
                let mut units = [0u16; 2];
                for &u in c.encode_utf16(&mut units).iter() {
                    out.push(tecla(u, KEYEVENTF_UNICODE));
                    out.push(tecla(u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                }
            }
        }
    }
}

impl InjetorTeclado for InjetorWindows {
    fn injetar(&mut self, eventos: &[Inject]) -> io::Result<()> {
        self.buf.clear();
        entradas(eventos, &mut self.buf);
        if self.buf.is_empty() {
            return Ok(());
        }
        // SAFETY: `buf` tem `len` estruturas INPUT válidas e o tamanho passado é o do tipo.
        let n = unsafe {
            SendInput(
                self.buf.len() as u32,
                self.buf.as_ptr(),
                size_of::<INPUT>() as i32,
            )
        };
        if n as usize == self.buf.len() {
            Ok(())
        } else {
            // UIPI bloqueia injeção numa janela elevada (UAC) a partir de um processo comum.
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ki(i: &INPUT) -> (u16, u32) {
        // SAFETY: todas as entradas montadas aqui são de teclado.
        let k = unsafe { i.Anonymous.ki };
        (k.wScan, k.dwFlags)
    }

    #[test]
    fn scancodes_e_flags() {
        let mut out = Vec::new();
        entradas(
            &[
                Inject::Key {
                    hid: 0x04,
                    down: true,
                },
                Inject::Key {
                    hid: 0x04,
                    down: false,
                },
                Inject::Key {
                    hid: 0xE6,
                    down: true,
                },
                Inject::Key {
                    hid: 0x87,
                    down: true,
                },
                Inject::Unicode('ß'),
                Inject::Unicode('😀'),
            ],
            &mut out,
        );
        let v: Vec<_> = out.iter().map(ki).collect();
        let s = KEYEVENTF_SCANCODE;
        let u = KEYEVENTF_UNICODE;
        assert_eq!(
            v,
            [
                (0x1E, s),
                (0x1E, s | KEYEVENTF_KEYUP),
                (0x38, s | KEYEVENTF_EXTENDEDKEY),
                (0x73, s),
                (0xDF, u),
                (0xDF, u | KEYEVENTF_KEYUP),
                (0xD83D, u),
                (0xD83D, u | KEYEVENTF_KEYUP),
                (0xDE00, u),
                (0xDE00, u | KEYEVENTF_KEYUP),
            ]
        );
    }
}
