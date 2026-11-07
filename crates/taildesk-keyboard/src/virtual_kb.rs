//! Teclado na tela do celular (`KeyboardInfo.source = VIRTUAL`, `mobile/android-detalhado.md`
//! §6–7, mockup 08).
//!
//! O teclado virtual não tem layout físico: o sistema (Gboard, SwiftKey) já entrega o texto
//! pronto, com acento e IME resolvidos. Então:
//! - texto vira [`KeyMsg::Char`] (o host acha as teclas no layout dele, `11-teclado.md` §2.3);
//! - com Ctrl, Alt ou Super travados na faixa de teclas, a próxima letra vira
//!   [`KeyMsg::Shortcut`] pela letra (Ctrl+C copia em qualquer layout do host);
//! - as teclas da faixa (Esc, Tab, setas, F1–F12…) não produzem texto e vão pela posição física
//!   ([`KeyMsg::Key`]), com os modificadores travados apertados em volta.
//!
//! Travas de um toque ([`Trava`]): um toque vale para a próxima tecla, dois toques prendem até
//! tocar de novo.

use crate::{Hid, KeyMsg, Mods};

/// Maior texto por `KeyChar` (o limite do fio, `taildesk_proto::control::limits::KEY_TEXT_LEN`).
pub const TEXTO_MAX: usize = taildesk_proto::control::limits::KEY_TEXT_LEN;

const L_CTRL: Hid = 0xE0;
const L_SHIFT: Hid = 0xE1;
const L_ALT: Hid = 0xE2;
const L_META: Hid = 0xE3;

/// Teclas da faixa acima do teclado virtual. Nenhuma produz texto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeclaFaixa {
    Esc,
    Tab,
    Enter,
    Backspace,
    Delete,
    Insert,
    Cima,
    Baixo,
    Esquerda,
    Direita,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 a F12.
    F(u8),
}

impl TeclaFaixa {
    /// HID usage (página 0x07). `F(n)` fora de 1..=12 vira F12.
    pub fn hid(self) -> Hid {
        match self {
            TeclaFaixa::Enter => 0x28,
            TeclaFaixa::Esc => 0x29,
            TeclaFaixa::Backspace => 0x2A,
            TeclaFaixa::Tab => 0x2B,
            TeclaFaixa::F(n) => 0x3A + Hid::from(n.clamp(1, 12) - 1),
            TeclaFaixa::Insert => 0x49,
            TeclaFaixa::Home => 0x4A,
            TeclaFaixa::PageUp => 0x4B,
            TeclaFaixa::Delete => 0x4C,
            TeclaFaixa::End => 0x4D,
            TeclaFaixa::PageDown => 0x4E,
            TeclaFaixa::Direita => 0x4F,
            TeclaFaixa::Esquerda => 0x50,
            TeclaFaixa::Baixo => 0x51,
            TeclaFaixa::Cima => 0x52,
        }
    }
}

/// Estado de um modificador na faixa.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Trava {
    #[default]
    Solta,
    /// Vale para a próxima tecla ou letra.
    UmaVez,
    /// Fica até tocar de novo.
    Travada,
}

impl Trava {
    fn proxima(self) -> Trava {
        match self {
            Trava::Solta => Trava::UmaVez,
            Trava::UmaVez => Trava::Travada,
            Trava::Travada => Trava::Solta,
        }
    }

    fn ativa(self) -> bool {
        self != Trava::Solta
    }
}

/// HID da tecla que produz `c` num teclado US, só como dica (`hid_hint`) para o host quando a
/// letra não existe no layout dele (`11-teclado.md` §2.3).
pub fn hid_us(c: char) -> Hid {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => 0x04 + (c as Hid - 'a' as Hid),
        c @ '1'..='9' => 0x1E + (c as Hid - '1' as Hid),
        '0' => 0x27,
        ' ' => 0x2C,
        '-' | '_' => 0x2D,
        '=' | '+' => 0x2E,
        '[' | '{' => 0x2F,
        ']' | '}' => 0x30,
        '\\' | '|' => 0x31,
        ';' | ':' => 0x33,
        '\'' | '"' => 0x34,
        '`' | '~' => 0x35,
        ',' | '<' => 0x36,
        '.' | '>' => 0x37,
        '/' | '?' => 0x38,
        _ => 0,
    }
}

/// O que um teclado físico ligado ao celular mandou ([`VirtualKeyboard::fisica`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fisica<'a> {
    /// Tecla sem texto (setas, Esc, F1–F12, Enter, Backspace…).
    Tecla(TeclaFaixa),
    /// O texto que a tecla produziu no layout do aparelho.
    Texto(&'a str),
}

/// Modificadores da faixa e a tradução do que o teclado virtual entrega.
#[derive(Clone, Debug, Default)]
pub struct VirtualKeyboard {
    ctrl: Trava,
    alt: Trava,
    meta: Trava,
    shift: Trava,
}

impl VirtualKeyboard {
    fn trava_mut(&mut self, m: Mods) -> Option<&mut Trava> {
        Some(match m {
            Mods::CTRL => &mut self.ctrl,
            Mods::ALT => &mut self.alt,
            Mods::META => &mut self.meta,
            Mods::SHIFT => &mut self.shift,
            _ => return None,
        })
    }

    /// Estado de um modificador (`Mods::CTRL`, `ALT`, `META` ou `SHIFT`).
    pub fn trava(&self, m: Mods) -> Trava {
        match m {
            Mods::CTRL => self.ctrl,
            Mods::ALT => self.alt,
            Mods::META => self.meta,
            Mods::SHIFT => self.shift,
            _ => Trava::Solta,
        }
    }

    /// Toque no botão do modificador: solta → uma vez → travada → solta.
    pub fn tocar(&mut self, m: Mods) {
        if let Some(t) = self.trava_mut(m) {
            *t = t.proxima();
        }
    }

    /// Modificadores ativos agora.
    pub fn mods(&self) -> Mods {
        let mut m = Mods::NONE;
        for (t, bit) in [
            (self.ctrl, Mods::CTRL),
            (self.alt, Mods::ALT),
            (self.meta, Mods::META),
            (self.shift, Mods::SHIFT),
        ] {
            if t.ativa() {
                m = m.with(bit);
            }
        }
        m
    }

    /// Algum modificador que faz atalho (Ctrl, Alt ou Super) está ativo.
    pub fn em_atalho(&self) -> bool {
        self.ctrl.ativa() || self.alt.ativa() || self.meta.ativa()
    }

    /// Depois de uma tecla: os de "uma vez" soltam.
    fn consumir(&mut self) {
        for t in [
            &mut self.ctrl,
            &mut self.alt,
            &mut self.meta,
            &mut self.shift,
        ] {
            if *t == Trava::UmaVez {
                *t = Trava::Solta;
            }
        }
    }

    /// Texto que o teclado do sistema entregou (uma letra, uma palavra do corretor, o resultado
    /// de um IME). `\n` vira Enter.
    pub fn texto(&mut self, s: &str, out: &mut Vec<KeyMsg>) {
        let mut resto = s;
        if self.em_atalho() {
            let mut chars = s.chars();
            let Some(c) = chars.next() else { return };
            resto = chars.as_str();
            if c == '\n' {
                self.tecla(TeclaFaixa::Enter, out);
            } else {
                self.atalho(c, out);
            }
        } else if self.shift == Trava::UmaVez {
            // Shift de um toque deixa a próxima letra maiúscula (o resto segue como veio).
            let mut chars = s.chars();
            if let Some(c) = chars.next() {
                let maiuscula: String = c.to_uppercase().collect();
                self.shift = Trava::Solta;
                self.texto_puro(&maiuscula, out);
                resto = chars.as_str();
            }
        }
        self.texto_puro(resto, out);
    }

    fn texto_puro(&mut self, s: &str, out: &mut Vec<KeyMsg>) {
        let travado_maiusculo = self.shift == Trava::Travada;
        for (i, linha) in s.split('\n').enumerate() {
            if i > 0 {
                self.tecla(TeclaFaixa::Enter, out);
            }
            let linha = if travado_maiusculo {
                linha.to_uppercase()
            } else {
                linha.to_owned()
            };
            let mut inicio = 0;
            while inicio < linha.len() {
                let mut fim = (inicio + TEXTO_MAX).min(linha.len());
                while !linha.is_char_boundary(fim) {
                    fim -= 1;
                }
                let pedaco = &linha[inicio..fim];
                let hid_hint = match pedaco.chars().next() {
                    Some(c) if pedaco.chars().count() == 1 => hid_us(c),
                    _ => 0,
                };
                out.push(KeyMsg::Char {
                    text: pedaco.to_owned(),
                    hid_hint,
                });
                inicio = fim;
            }
        }
    }

    fn atalho(&mut self, c: char, out: &mut Vec<KeyMsg>) {
        let mut mods = self.mods();
        // A letra maiúscula do teclado virtual pede o Shift do atalho (Ctrl+Shift+T).
        let base = if c.is_uppercase() {
            mods = mods.with(Mods::SHIFT);
            c.to_lowercase().next().unwrap_or(c)
        } else {
            c
        };
        let hid_hint = hid_us(base);
        for down in [true, false] {
            out.push(KeyMsg::Shortcut {
                base_char: base,
                hid_hint,
                mods,
                down,
            });
        }
        self.consumir();
    }

    /// Toque numa tecla da faixa: os modificadores ativos ficam apertados em volta dela.
    pub fn tecla(&mut self, t: TeclaFaixa, out: &mut Vec<KeyMsg>) {
        let mods: Vec<Hid> = [
            (self.ctrl, L_CTRL),
            (self.alt, L_ALT),
            (self.meta, L_META),
            (self.shift, L_SHIFT),
        ]
        .into_iter()
        .filter(|(t, _)| t.ativa())
        .map(|(_, h)| h)
        .collect();
        let key = |hid, down| KeyMsg::Key {
            hid,
            down,
            locks: None,
        };
        out.extend(mods.iter().map(|&h| key(h, true)));
        out.push(key(t.hid(), true));
        out.push(key(t.hid(), false));
        out.extend(mods.iter().rev().map(|&h| key(h, false)));
        self.consumir();
    }

    /// Tecla de um teclado físico (USB ou Bluetooth) ligado ao celular, com os modificadores
    /// apertados junto. O Android já aplicou o layout do aparelho, então segue o modo Auto de
    /// `11-teclado.md`: tecla sem texto pela posição física (com os modificadores em volta),
    /// atalho pela letra, e o resto como caractere. Cada aperto vira um toque completo (descer e
    /// subir); a repetição do sistema manda outro.
    pub fn fisica(&mut self, k: Fisica<'_>, apertados: Mods, out: &mut Vec<KeyMsg>) {
        let segurar = |t: &mut Trava, m: Mods| {
            if apertados.contains(m) && *t == Trava::Solta {
                *t = Trava::UmaVez;
            }
        };
        segurar(&mut self.ctrl, Mods::CTRL);
        segurar(&mut self.alt, Mods::ALT);
        segurar(&mut self.meta, Mods::META);
        match k {
            Fisica::Tecla(t) => {
                segurar(&mut self.shift, Mods::SHIFT);
                self.tecla(t, out);
            }
            Fisica::Texto(s) if self.em_atalho() => {
                let Some(c) = s.chars().next() else {
                    self.consumir();
                    return;
                };
                // Com Ctrl o sistema pode entregar o caractere de controle (Ctrl+C = U+0003).
                let c = match c {
                    '\u{1}'..='\u{1a}' => char::from(b'a' + (c as u8) - 1),
                    c => c,
                };
                // O Shift apertado faz parte do atalho (Ctrl+Shift+T), mesmo se a letra veio
                // minúscula.
                let c = if apertados.contains(Mods::SHIFT) {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c
                };
                self.atalho(c, out);
            }
            // O Shift já está no texto (o Android aplicou).
            Fisica::Texto(s) => self.texto(s, out),
        }
    }

    /// Apagar uma letra que o teclado do sistema pediu (backspace do IME).
    pub fn apagar(&mut self, quantas: usize, out: &mut Vec<KeyMsg>) {
        for _ in 0..quantas.min(TEXTO_MAX) {
            self.tecla(TeclaFaixa::Backspace, out);
        }
    }

    /// Saiu da sessão ou foi para segundo plano: solta tudo, aqui e no host.
    pub fn soltar_tudo(&mut self, out: &mut Vec<KeyMsg>) {
        *self = VirtualKeyboard::default();
        out.push(KeyMsg::ReleaseAll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn travas_de_um_toque() {
        let mut v = VirtualKeyboard::default();
        v.tocar(Mods::CTRL);
        assert_eq!(v.trava(Mods::CTRL), Trava::UmaVez);
        let mut out = Vec::new();
        v.texto("c", &mut out);
        assert_eq!(
            out,
            vec![
                KeyMsg::Shortcut {
                    base_char: 'c',
                    hid_hint: 0x06,
                    mods: Mods::CTRL,
                    down: true
                },
                KeyMsg::Shortcut {
                    base_char: 'c',
                    hid_hint: 0x06,
                    mods: Mods::CTRL,
                    down: false
                },
            ]
        );
        // Uma vez só: a próxima letra é texto.
        assert_eq!(v.trava(Mods::CTRL), Trava::Solta);
        out.clear();
        v.texto("c", &mut out);
        assert!(matches!(&out[..], [KeyMsg::Char { text, .. }] if text == "c"));

        // Dois toques prendem; o terceiro solta.
        v.tocar(Mods::ALT);
        v.tocar(Mods::ALT);
        out.clear();
        v.tecla(TeclaFaixa::Tab, &mut out);
        v.tecla(TeclaFaixa::Tab, &mut out);
        assert_eq!(v.trava(Mods::ALT), Trava::Travada);
        assert_eq!(out.len(), 8);
        v.tocar(Mods::ALT);
        assert_eq!(v.mods(), Mods::NONE);
    }

    #[test]
    fn texto_grande_e_quebras_de_linha() {
        let mut v = VirtualKeyboard::default();
        let mut out = Vec::new();
        let longo = "ç".repeat(TEXTO_MAX);
        v.texto(&format!("{longo}\nfim"), &mut out);
        let textos: Vec<&str> = out
            .iter()
            .filter_map(|m| match m {
                KeyMsg::Char { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(textos.iter().all(|t| t.len() <= TEXTO_MAX));
        assert_eq!(textos.concat(), format!("{longo}fim"));
        assert!(out.contains(&KeyMsg::Key {
            hid: 0x28,
            down: true,
            locks: None
        }));
    }

    #[test]
    fn faixa_aperta_os_modificadores_em_volta() {
        let mut v = VirtualKeyboard::default();
        v.tocar(Mods::CTRL);
        v.tocar(Mods::ALT);
        let mut out = Vec::new();
        v.tecla(TeclaFaixa::F(4), &mut out);
        let hids: Vec<(Hid, bool)> = out
            .iter()
            .map(|m| match m {
                KeyMsg::Key { hid, down, .. } => (*hid, *down),
                outro => panic!("{outro:?}"),
            })
            .collect();
        assert_eq!(
            hids,
            vec![
                (L_CTRL, true),
                (L_ALT, true),
                (0x3D, true),
                (0x3D, false),
                (L_ALT, false),
                (L_CTRL, false),
            ]
        );
        assert_eq!(v.mods(), Mods::NONE);
        assert_eq!(TeclaFaixa::F(1).hid(), 0x3A);
        assert_eq!(TeclaFaixa::F(12).hid(), 0x45);
    }

    #[test]
    fn shift_de_um_toque_so_na_primeira_letra() {
        let mut v = VirtualKeyboard::default();
        v.tocar(Mods::SHIFT);
        let mut out = Vec::new();
        v.texto("ação", &mut out);
        let texto: String = out
            .iter()
            .filter_map(|m| match m {
                KeyMsg::Char { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texto, "Ação");
        assert_eq!(v.mods(), Mods::NONE);
    }
}
