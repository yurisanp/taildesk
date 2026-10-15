//! Tradução automática entre layouts de teclado (`docs/context/11-teclado.md`).
//!
//! Lógica pura, sem SO: os leitores de layout (`ToUnicodeEx` no Windows, `xkbcommon` no Linux)
//! ficam em `taildesk-platform` e produzem um [`Keymap`]. Aqui ficam:
//! - [`classify`]: no controlador, decide se a tecla vai como física, atalho ou caractere;
//! - [`ReverseMap`]: no host, descobre quais teclas do layout local produzem um caractere;
//! - [`Controller`]: no controlador, transforma eventos de tecla em [`KeyMsg`] (teclas mortas já
//!   resolvidas);
//! - [`Host`]: no host, transforma [`KeyMsg`] em teclas a injetar ([`Inject`]) no layout local.

use std::collections::HashMap;

pub mod codes;
mod controller;
mod dump;
pub mod fixtures;
mod host;
mod msg;
mod wire;

pub use controller::Controller;
pub use host::{Host, Inject};
pub use msg::{KeyMsg, Locks, Mode, Translation, effective_translation, layout_tag};

/// Código de tecla no fio: USB HID usage (página 0x07).
pub type Hid = u16;

/// Modificadores ativos, como bitmask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const SHIFT: Mods = Mods(1);
    pub const CTRL: Mods = Mods(1 << 1);
    pub const ALT: Mods = Mods(1 << 2);
    pub const META: Mods = Mods(1 << 3);
    /// AltGr. No Windows o leitor de input deve converter o par sintético
    /// `LCtrl` + `RAlt` em `ALTGR` antes de chegar aqui.
    pub const ALTGR: Mods = Mods(1 << 4);

    /// Bits no fio (`KeyShortcut.mods`): 0 Shift, 1 Ctrl, 2 Alt, 3 Meta, 4 AltGr.
    pub const fn bits(self) -> u32 {
        self.0 as u32
    }

    /// Ignora bits desconhecidos.
    pub const fn from_bits(bits: u32) -> Mods {
        Mods((bits & 0x1F) as u8)
    }

    pub const fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Mods) -> bool {
        self.0 & other.0 != 0
    }

    #[must_use]
    pub const fn with(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }
}

/// Como o controlador deve mandar uma tecla quando os layouts diferem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyClass {
    /// Posição física (`Key{hid}`): teclas que não produzem texto.
    Physical,
    /// Atalho pela letra (`KeyShortcut{base_char}`): Ctrl/Alt/Meta + tecla, sem AltGr.
    Shortcut,
    /// Caractere já resolvido no controlador (`KeyChar{text}`).
    Char,
}

/// Teclas que nunca produzem texto, em qualquer layout.
pub fn is_non_printing(hid: Hid) -> bool {
    matches!(hid,
        0x28..=0x2B          // Enter, Esc, Backspace, Tab
        | 0x39..=0x53        // CapsLock, F1-F12, PrintScreen..setas, NumLock
        | 0x58               // Keypad Enter
        | 0x65..=0x66        // Application (menu), Power
        | 0x68..=0x73        // F13-F24
        | 0x74..=0x84        // Execute..Volume, LockingNumLock, LockingScrollLock
        | 0x86               // Keypad = (AS/400). 0x85 (Keypad Comma) produz texto no ABNT2.
        | 0xE0..=0xE7        // modificadores
    )
}

/// Teclas do teclado numérico (dependem do Num Lock ou ficam em posição fixa em todo layout).
pub fn is_keypad(hid: Hid) -> bool {
    matches!(hid, 0x54..=0x63 | 0x67 | 0x85)
}

/// Modificadores sozinhos (Ctrl, Shift, Alt, Meta, esquerdo e direito).
pub fn is_modifier(hid: Hid) -> bool {
    matches!(hid, 0xE0..=0xE7)
}

/// Tecla de navegação que uma tecla do teclado numérico vira com o Num Lock desligado.
pub fn keypad_nav(hid: Hid) -> Option<Hid> {
    Some(match hid {
        0x59 => 0x4D, // 1 -> End
        0x5A => 0x51, // 2 -> Down
        0x5B => 0x4E, // 3 -> PageDown
        0x5C => 0x50, // 4 -> Left
        0x5E => 0x4F, // 6 -> Right
        0x5F => 0x4A, // 7 -> Home
        0x60 => 0x52, // 8 -> Up
        0x61 => 0x4B, // 9 -> PageUp
        0x62 => 0x49, // 0 -> Insert
        0x63 => 0x4C, // . -> Delete
        _ => return None,
    })
}

/// Classificação do modo Auto (`11-teclado.md` §2.3).
///
/// `produces_text` diz se, com os modificadores atuais, a tecla gera texto no layout do
/// controlador (inclui o caso de tecla morta que vai compor).
pub fn classify(hid: Hid, mods: Mods, produces_text: bool) -> KeyClass {
    if is_non_printing(hid)
        || !produces_text && !mods.intersects(Mods::CTRL.with(Mods::ALT).with(Mods::META))
    {
        return KeyClass::Physical;
    }
    if !mods.contains(Mods::ALTGR) && mods.intersects(Mods::CTRL.with(Mods::ALT).with(Mods::META)) {
        return KeyClass::Shortcut;
    }
    if produces_text {
        KeyClass::Char
    } else {
        KeyClass::Physical
    }
}

/// Nível de shift dentro de uma tecla.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    Base,
    Shift,
    AltGr,
    ShiftAltGr,
}

impl Level {
    /// Nível a partir de Shift e AltGr.
    pub fn from_mods(mods: Mods) -> Level {
        match (mods.contains(Mods::SHIFT), mods.contains(Mods::ALTGR)) {
            (false, false) => Level::Base,
            (true, false) => Level::Shift,
            (false, true) => Level::AltGr,
            (true, true) => Level::ShiftAltGr,
        }
    }

    /// Troca base por Shift (efeito do Caps Lock em letras). AltGr não muda.
    pub fn caps_flipped(self) -> Level {
        match self {
            Level::Base => Level::Shift,
            Level::Shift => Level::Base,
            other => other,
        }
    }

    fn cost(self) -> u32 {
        match self {
            Level::Base => 0,
            Level::Shift => 1,
            Level::AltGr => 3,
            Level::ShiftAltGr => 4,
        }
    }
}

/// O que uma tecla produz num nível.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Char(char),
    /// Tecla morta; o `char` é o acento "solto" (ex.: `´`, `~`, `'` no US-Intl).
    Dead(char),
}

/// Layout lido do SO: o que cada tecla produz em cada nível, mais as composições de teclas mortas.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    pub name: String,
    pub keys: Vec<(Hid, Level, Output)>,
    /// `(morta, base, resultado)`. A base `' '` dá o acento sozinho.
    pub compose: Vec<(char, char, char)>,
}

/// Uma tecla a apertar (com o nível já implicando Shift/AltGr).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stroke {
    pub hid: Hid,
    pub level: Level,
}

/// Busca reversa: caractere -> sequência de teclas mais simples no layout local.
#[derive(Clone, Debug, Default)]
pub struct ReverseMap {
    map: HashMap<char, Vec<Stroke>>,
}

fn sequence_cost(seq: &[Stroke]) -> u32 {
    // O teclado numérico só digita com o Num Lock ligado no host: fica como última opção.
    seq.iter()
        .map(|s| 10 + s.level.cost() + if is_keypad(s.hid) { 20 } else { 0 })
        .sum()
}

impl ReverseMap {
    pub fn build(keymap: &Keymap) -> Self {
        let mut map: HashMap<char, Vec<Stroke>> = HashMap::new();
        let mut dead: HashMap<char, Stroke> = HashMap::new();
        for &(hid, level, out) in &keymap.keys {
            let stroke = Stroke { hid, level };
            match out {
                Output::Char(c) => offer(&mut map, c, vec![stroke]),
                Output::Dead(d) => {
                    let e = dead.entry(d).or_insert(stroke);
                    if (level.cost(), hid) < (e.level.cost(), e.hid) {
                        *e = stroke;
                    }
                }
            }
        }
        let direct = map.clone();
        for &(d, base, result) in &keymap.compose {
            let Some(&dk) = dead.get(&d) else {
                continue;
            };
            // A segunda tecla pode ser um caractere direto ou outra morta (`´` + `´` = `´`).
            let mut seq = vec![dk];
            match (direct.get(&base), dead.get(&base)) {
                (Some(base_seq), _) => seq.extend_from_slice(base_seq),
                (None, Some(&bk)) => seq.push(bk),
                (None, None) => continue,
            }
            offer(&mut map, result, seq);
        }
        ReverseMap { map }
    }

    /// Teclas que produzem `c` no layout local, ou `None` se o caractere não existe nele.
    pub fn lookup(&self, c: char) -> Option<&[Stroke]> {
        self.map.get(&c).map(Vec::as_slice)
    }
}

/// Guarda `seq` para `c` se ela for mais simples que a atual.
fn offer(map: &mut HashMap<char, Vec<Stroke>>, c: char, seq: Vec<Stroke>) {
    let better = match map.get(&c) {
        Some(cur) => (sequence_cost(&seq), seq_key(&seq)) < (sequence_cost(cur), seq_key(cur)),
        None => true,
    };
    if better {
        map.insert(c, seq);
    }
}

/// Desempate determinístico entre sequências de mesmo custo.
fn seq_key(seq: &[Stroke]) -> Vec<(Level, Hid)> {
    seq.iter().map(|s| (s.level, s.hid)).collect()
}

impl Keymap {
    /// O que a tecla produz no nível, se produz algo.
    pub fn output(&self, hid: Hid, level: Level) -> Option<Output> {
        self.keys
            .iter()
            .find_map(|&(h, l, o)| (h == hid && l == level).then_some(o))
    }

    /// A tecla muda com o Caps Lock: o nível base é uma letra com maiúscula no nível Shift.
    pub fn caps_affects(&self, hid: Hid) -> bool {
        match (
            self.output(hid, Level::Base),
            self.output(hid, Level::Shift),
        ) {
            (Some(Output::Char(b)), Some(Output::Char(s))) => {
                b.is_alphabetic() && b != s && b.to_uppercase().eq(std::iter::once(s))
            }
            _ => false,
        }
    }

    /// Resultado da composição de `dead` com `base`.
    pub fn compose(&self, dead: char, base: char) -> Option<char> {
        self.compose
            .iter()
            .find_map(|&(d, b, r)| (d == dead && b == base).then_some(r))
    }
}

/// Caractere que uma tecla produz no nível base, para atalhos (`KeyShortcut.base_char`).
pub fn base_char(keymap: &Keymap, hid: Hid) -> Option<char> {
    keymap
        .keys
        .iter()
        .find_map(|&(h, level, out)| match (h == hid, level, out) {
            (true, Level::Base, Output::Char(c)) => Some(c.to_lowercase().next().unwrap_or(c)),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::fixtures::{abnt2, us_intl_windows};
    use super::*;

    fn s(hid: Hid, level: Level) -> Stroke {
        Stroke { hid, level }
    }

    #[test]
    fn abnt2_direct_keys() {
        let rm = ReverseMap::build(&abnt2());
        assert_eq!(rm.lookup('ç'), Some(&[s(0x33, Level::Base)][..]));
        assert_eq!(rm.lookup('Ç'), Some(&[s(0x33, Level::Shift)][..]));
        assert_eq!(rm.lookup('"'), Some(&[s(0x35, Level::Shift)][..]));
        assert_eq!(rm.lookup(';'), Some(&[s(0x38, Level::Base)][..]));
    }

    #[test]
    fn abnt2_prefers_dedicated_key_over_altgr() {
        let rm = ReverseMap::build(&abnt2());
        // `/` existe na tecla 0x87 e em AltGr+Q; a tecla dedicada é mais simples.
        assert_eq!(rm.lookup('/'), Some(&[s(0x87, Level::Base)][..]));
        // `°` só existe via AltGr+E.
        assert_eq!(rm.lookup('°'), Some(&[s(0x08, Level::AltGr)][..]));
    }

    #[test]
    fn abnt2_dead_key_sequences() {
        let rm = ReverseMap::build(&abnt2());
        assert_eq!(
            rm.lookup('é'),
            Some(&[s(0x2F, Level::Base), s(0x08, Level::Base)][..])
        );
        assert_eq!(
            rm.lookup('ã'),
            Some(&[s(0x34, Level::Base), s(0x04, Level::Base)][..])
        );
        assert_eq!(
            rm.lookup('Ê'),
            Some(&[s(0x34, Level::Shift), s(0x08, Level::Shift)][..])
        );
        assert_eq!(
            rm.lookup('ü'),
            Some(&[s(0x23, Level::Shift), s(0x18, Level::Base)][..])
        );
        // Acento sozinho = morta + espaço.
        assert_eq!(
            rm.lookup('^'),
            Some(&[s(0x34, Level::Shift), s(0x2C, Level::Base)][..])
        );
    }

    #[test]
    fn missing_char_is_none() {
        assert_eq!(ReverseMap::build(&abnt2()).lookup('ß'), None);
    }

    /// Cenário do requisito: Windows US-Intl controlando Linux ABNT2.
    /// O controlador resolve `'` + `c` como `ç` (comportamento do Windows US-Intl); o host ABNT2
    /// aperta a tecla do `ç`, não a tecla `'`+`c` na mesma posição física.
    #[test]
    fn us_intl_to_abnt2() {
        let us = ReverseMap::build(&us_intl_windows());
        let br = ReverseMap::build(&abnt2());
        // No US-Intl o caminho mais curto para `ç` é AltGr+`,`; `'` + `c` também compõe `ç`.
        assert_eq!(us.lookup('ç'), Some(&[s(0x36, Level::AltGr)][..]));
        assert!(us_intl_windows().compose.contains(&('\'', 'c', 'ç')));
        assert_eq!(br.lookup('ç'), Some(&[s(0x33, Level::Base)][..]));
        // `é` no ABNT2 precisa da morta `´`, que fica onde o US tem `[`.
        assert_eq!(
            br.lookup('é'),
            Some(&[s(0x2F, Level::Base), s(0x08, Level::Base)][..])
        );
        // `'` no US-Intl é morta + espaço; no ABNT2 é a tecla 0x35 direta.
        assert_eq!(
            us.lookup('\''),
            Some(&[s(0x34, Level::Base), s(0x2C, Level::Base)][..])
        );
        assert_eq!(br.lookup('\''), Some(&[s(0x35, Level::Base)][..]));
        // E o contrário: `/` digitado na tecla 0x87 do ABNT2 vira a tecla 0x38 no US.
        assert_eq!(us.lookup('/'), Some(&[s(0x38, Level::Base)][..]));
    }

    #[test]
    fn shortcut_uses_letter_from_controller_layout() {
        assert_eq!(base_char(&us_intl_windows(), 0x06), Some('c'));
        assert_eq!(base_char(&abnt2(), 0x33), Some('ç'));
        assert_eq!(base_char(&abnt2(), 0x28), None);
    }

    #[test]
    fn classification() {
        let ctrl = Mods::CTRL;
        assert_eq!(classify(0x28, Mods::NONE, false), KeyClass::Physical); // Enter
        assert_eq!(classify(0x52, Mods::SHIFT, false), KeyClass::Physical); // Shift+Up
        assert_eq!(classify(0x06, ctrl, true), KeyClass::Shortcut); // Ctrl+C
        assert_eq!(classify(0x14, Mods::ALTGR, true), KeyClass::Char); // AltGr+Q = `/`
        assert_eq!(
            classify(0x14, Mods::ALTGR.with(Mods::CTRL), true),
            KeyClass::Char
        );
        assert_eq!(classify(0x08, Mods::SHIFT, true), KeyClass::Char); // `E`
        assert_eq!(classify(0x2F, Mods::NONE, true), KeyClass::Char); // morta `´`
        assert_eq!(classify(0x3A, ctrl, false), KeyClass::Physical); // Ctrl+F1
        assert_eq!(classify(0xE1, Mods::SHIFT, false), KeyClass::Physical); // Shift sozinho
    }
}
