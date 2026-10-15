//! Leitura do layout no Windows com `ToUnicodeEx` (`11-teclado.md` §2.1).
//!
//! No Windows o layout é por thread: o controlador lê o da própria janela
//! ([`ler_keymap_ativo`]) e o host o da janela em foco, onde as teclas vão cair
//! ([`ler_keymap_da_thread`]). A composição de teclas mortas mexe no estado de teclado da thread
//! que chama, então chame de uma thread de trabalho, nunca da thread da UI.

use taildesk_keyboard::codes::{hid_to_scancode, known_hids};
use taildesk_keyboard::{Hid, Keymap, Level, Output, is_keypad, is_non_printing};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ActivateKeyboardLayout, GetKeyboardLayout, GetKeyboardLayoutNameW, HKL, KLF_NOTELLSHELL,
    LoadKeyboardLayoutW, MAPVK_VSC_TO_VK_EX, MapVirtualKeyExW, ToUnicodeEx, VK_CONTROL, VK_DECIMAL,
    VK_LCONTROL, VK_LSHIFT, VK_MENU, VK_NUMLOCK, VK_NUMPAD0, VK_NUMPAD1, VK_RMENU, VK_SHIFT,
    VK_SPACE,
};

/// Erro ao ler o layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErroLayout {
    /// `LoadKeyboardLayoutW` recusou o KLID (layout não instalado ou id inválido).
    LayoutInvalido,
}

impl std::fmt::Display for ErroLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("layout de teclado inválido")
    }
}

impl std::error::Error for ErroLayout {}

/// `ToUnicodeEx` sem mexer no estado de teclado da thread (Windows 10 1607+).
const SEM_MUDAR_ESTADO: u32 = 1 << 2;

/// Layout da thread que chama (a janela da sessão, no controlador).
pub fn ler_keymap_ativo() -> Keymap {
    // SAFETY: sem pré-condições; 0 = thread atual.
    let hkl = unsafe { GetKeyboardLayout(0) };
    ler(hkl, klid_ativo())
}

/// Layout de outra thread (a da janela em foco, no host).
pub fn ler_keymap_da_thread(thread: u32) -> Keymap {
    // SAFETY: sem pré-condições; thread inexistente devolve o layout padrão.
    let hkl = unsafe { GetKeyboardLayout(thread) };
    ler(hkl, klid_de(hkl))
}

/// KLID de um HKL: o nome só sai do layout ativo da thread atual, então ativa, lê e volta ao
/// anterior.
fn klid_de(hkl: HKL) -> String {
    // SAFETY: `hkl` veio do sistema; o anterior é restaurado logo em seguida.
    unsafe {
        let anterior = ActivateKeyboardLayout(hkl, 0);
        let nome = klid_ativo();
        if !anterior.is_null() {
            ActivateKeyboardLayout(anterior, 0);
        }
        nome
    }
}

/// Carrega um layout pelo KLID (`"00020409"` = US-Intl, `"00010416"` = ABNT2) e lê.
pub fn ler_keymap_klid(klid: &str) -> Result<Keymap, ErroLayout> {
    let largo: Vec<u16> = klid.encode_utf16().chain(Some(0)).collect();
    // SAFETY: string UTF-16 terminada em NUL e viva durante a chamada.
    let hkl = unsafe { LoadKeyboardLayoutW(largo.as_ptr(), KLF_NOTELLSHELL) };
    // Com KLID desconhecido o Windows devolve o layout padrão em vez de falhar.
    if hkl.is_null() || !klid_de(hkl).eq_ignore_ascii_case(klid) {
        return Err(ErroLayout::LayoutInvalido);
    }
    Ok(ler(hkl, klid.to_string()))
}

fn klid_ativo() -> String {
    let mut buf = [0u16; 9];
    // SAFETY: o buffer tem KL_NAMELENGTH (9) posições.
    let ok = unsafe { GetKeyboardLayoutNameW(buf.as_mut_ptr()) };
    if ok == 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..8])
}

/// Uma tecla pronta para `ToUnicodeEx`.
#[derive(Clone, Copy)]
struct Tecla {
    vk: u32,
    scan: u32,
    estado: [u8; 256],
}

fn estado(level: Level, numerico: bool) -> [u8; 256] {
    let mut e = [0u8; 256];
    if matches!(level, Level::Shift | Level::ShiftAltGr) {
        e[usize::from(VK_SHIFT)] = 0x80;
        e[usize::from(VK_LSHIFT)] = 0x80;
    }
    // AltGr é Ctrl + Alt no Windows.
    if matches!(level, Level::AltGr | Level::ShiftAltGr) {
        e[usize::from(VK_CONTROL)] = 0x80;
        e[usize::from(VK_LCONTROL)] = 0x80;
        e[usize::from(VK_MENU)] = 0x80;
        e[usize::from(VK_RMENU)] = 0x80;
    }
    if numerico {
        e[usize::from(VK_NUMLOCK)] = 0x01;
    }
    e
}

/// `ToUnicodeEx`: o código de retorno (-1 = tecla morta) e o caractere, se for um só.
fn traduz(t: &Tecla, flags: u32, hkl: HKL) -> (i32, Option<char>) {
    let mut buf = [0u16; 8];
    // SAFETY: estado com 256 posições e buffer com o tamanho informado.
    let rc = unsafe {
        ToUnicodeEx(
            t.vk,
            t.scan,
            t.estado.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as i32,
            flags,
            hkl,
        )
    };
    let n = match rc {
        -1 => 1,
        n if n > 0 => n as usize,
        _ => return (rc, None),
    };
    let mut it = char::decode_utf16(buf[..n.min(buf.len())].iter().copied());
    let c = match (it.next(), it.next()) {
        (Some(Ok(c)), None) => Some(c),
        _ => None,
    };
    (rc, c)
}

fn ler(hkl: HKL, nome: String) -> Keymap {
    let mut keys = Vec::new();
    let mut mortas: Vec<(char, Tecla)> = Vec::new();
    let mut bases: Vec<(char, Tecla)> = Vec::new();
    for level in [Level::Base, Level::Shift, Level::AltGr, Level::ShiftAltGr] {
        for hid in known_hids() {
            // 0x32 e 0x31 são a mesma tecla física (scancode 0x2B).
            if hid == 0x32 || is_non_printing(hid) {
                continue;
            }
            let Some(t) = tecla(hid, level, hkl) else {
                continue;
            };
            let saida = match traduz(&t, SEM_MUDAR_ESTADO, hkl) {
                (-1, Some(c)) => Output::Dead(c),
                (1.., Some(c)) if !c.is_control() => Output::Char(c),
                _ => continue,
            };
            match saida {
                Output::Dead(c) if !mortas.iter().any(|&(m, _)| m == c) => mortas.push((c, t)),
                Output::Char(c) if !bases.iter().any(|&(b, _)| b == c) => bases.push((c, t)),
                _ => {}
            }
            keys.push((hid, level, saida));
        }
    }
    let compose = composicoes(hkl, &mortas, &bases);
    Keymap {
        name: nome,
        keys,
        compose,
    }
}

fn tecla(hid: Hid, level: Level, hkl: HKL) -> Option<Tecla> {
    let scan = u32::from(hid_to_scancode(hid)?);
    // O scancode do numérico mapeia para a tecla de navegação (VK_END...); com Num Lock ligado
    // a tecla virtual é VK_NUMPADn / VK_DECIMAL.
    let vk = match hid {
        0x59..=0x61 => u32::from(VK_NUMPAD1) + u32::from(hid - 0x59),
        0x62 => u32::from(VK_NUMPAD0),
        0x63 => u32::from(VK_DECIMAL),
        // SAFETY: função pura de tradução; 0 = sem tecla virtual.
        _ => unsafe { MapVirtualKeyExW(scan, MAPVK_VSC_TO_VK_EX, hkl) },
    };
    (vk != 0).then(|| Tecla {
        vk,
        scan: scan & 0xFF,
        estado: estado(level, is_keypad(hid)),
    })
}

/// Composição de cada morta com cada caractere do layout, com espaço e com outra morta.
/// Usa o estado de teclado da thread atual e o deixa limpo no fim.
fn composicoes(
    hkl: HKL,
    mortas: &[(char, Tecla)],
    bases: &[(char, Tecla)],
) -> Vec<(char, char, char)> {
    let espaco = Tecla {
        vk: u32::from(VK_SPACE),
        scan: 0x39,
        estado: [0; 256],
    };
    let mut out = Vec::new();
    for (m, tm) in mortas {
        for (b, tb) in bases.iter().chain(mortas) {
            if traduz(tm, 0, hkl).0 != -1 {
                continue;
            }
            let (rc, c) = traduz(tb, 0, hkl);
            if let (1, Some(r)) = (rc, c) {
                out.push((*m, *b, r));
            }
            if rc == -1 {
                // A segunda morta ficou guardada: o espaço a consome.
                traduz(&espaco, 0, hkl);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use taildesk_keyboard::fixtures::{abnt2, us, us_intl_windows};

    /// Os fixtures escritos à mão têm que bater com o Windows de verdade.
    fn confere(klid: &str, esperado: Keymap) {
        let real = ler_keymap_klid(klid).unwrap();
        assert_eq!(real.name, klid);
        let mut erros = Vec::new();
        for &(hid, level, out) in &esperado.keys {
            let lido = real.output(hid, level);
            if lido != Some(out) {
                erros.push(format!(
                    "tecla {hid:#04x} {level:?}: fixture {out:?}, Windows {lido:?}"
                ));
            }
        }
        for &(d, b, r) in &esperado.compose {
            let lido = real.compose(d, b);
            if lido != Some(r) {
                erros.push(format!(
                    "composição {d} + {b:?}: fixture {r}, Windows {lido:?}"
                ));
            }
        }
        assert!(erros.is_empty(), "{klid}:\n{}", erros.join("\n"));
    }

    #[test]
    fn us_intl_do_windows() {
        confere("00020409", us_intl_windows());
    }

    #[test]
    fn abnt2_do_windows() {
        confere("00010416", abnt2());
    }

    #[test]
    fn us_do_windows() {
        confere("00000409", us());
    }

    #[test]
    fn klid_invalido() {
        assert_eq!(
            ler_keymap_klid("zzzzzzzz").map(|_| ()),
            Err(ErroLayout::LayoutInvalido)
        );
    }
}
