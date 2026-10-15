//! Formato de texto dos layouts gravados como fixtures (`11-teclado.md` §5).
//!
//! Uma linha por fato; caracteres em `U+XXXX` para espaços e acentos sobreviverem a editores,
//! e o caractere literal depois de `#` só para leitura:
//! ```text
//! name br-abnt2
//! key 0x33 base char U+00E7 # ç
//! key 0x2f base dead U+00B4 # ´
//! compose U+00B4 U+0065 U+00E9 # ´ e é
//! ```

use std::fmt::Write as _;

use crate::{Hid, Keymap, Level, Output};

/// Limite de linhas ao ler um dump (um layout real tem algumas centenas).
const MAX_LINES: usize = 20_000;

fn level_name(l: Level) -> &'static str {
    match l {
        Level::Base => "base",
        Level::Shift => "shift",
        Level::AltGr => "altgr",
        Level::ShiftAltGr => "shift-altgr",
    }
}

fn shown(c: char) -> char {
    if c.is_control() || c.is_whitespace() {
        '·'
    } else {
        c
    }
}

impl Keymap {
    /// Grava o layout no formato de fixture.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "name {}", self.name);
        for &(hid, level, out) in &self.keys {
            let (kind, c) = match out {
                Output::Char(c) => ("char", c),
                Output::Dead(c) => ("dead", c),
            };
            let _ = writeln!(
                s,
                "key {hid:#04x} {} {kind} U+{:04X} # {}",
                level_name(level),
                c as u32,
                shown(c)
            );
        }
        for &(d, b, r) in &self.compose {
            let _ = writeln!(
                s,
                "compose U+{:04X} U+{:04X} U+{:04X} # {} {} {}",
                d as u32,
                b as u32,
                r as u32,
                shown(d),
                shown(b),
                shown(r)
            );
        }
        s
    }

    /// Lê um layout gravado com [`Keymap::to_text`].
    pub fn from_text(text: &str) -> Result<Keymap, String> {
        let mut km = Keymap::default();
        for (n, line) in text.lines().enumerate() {
            if n >= MAX_LINES {
                return Err(format!("mais de {MAX_LINES} linhas"));
            }
            let line = line.split('#').next().unwrap_or("").trim();
            let err = |what: &str| format!("linha {}: {what}: {line:?}", n + 1);
            let f: Vec<&str> = line.split_whitespace().collect();
            match f.as_slice() {
                [] => {}
                ["name", name] => km.name = (*name).to_string(),
                ["key", hid, level, kind, c] => {
                    let hid = hid
                        .strip_prefix("0x")
                        .and_then(|h| Hid::from_str_radix(h, 16).ok())
                        .ok_or_else(|| err("tecla"))?;
                    let level = match *level {
                        "base" => Level::Base,
                        "shift" => Level::Shift,
                        "altgr" => Level::AltGr,
                        "shift-altgr" => Level::ShiftAltGr,
                        _ => return Err(err("nível")),
                    };
                    let c = parse_char(c).ok_or_else(|| err("caractere"))?;
                    let out = match *kind {
                        "char" => Output::Char(c),
                        "dead" => Output::Dead(c),
                        _ => return Err(err("tipo")),
                    };
                    km.keys.push((hid, level, out));
                }
                ["compose", d, b, r] => {
                    let p = |s: &str| parse_char(s).ok_or_else(|| err("caractere"));
                    km.compose.push((p(d)?, p(b)?, p(r)?));
                }
                _ => return Err(err("linha inválida")),
            }
        }
        Ok(km)
    }
}

fn parse_char(s: &str) -> Option<char> {
    char::from_u32(u32::from_str_radix(s.strip_prefix("U+")?, 16).ok()?)
}

#[cfg(test)]
mod tests {
    use crate::Keymap;
    use crate::fixtures::{abnt2, us, us_intl_windows};

    #[test]
    fn round_trip() {
        for km in [abnt2(), us(), us_intl_windows()] {
            let text = km.to_text();
            let back = Keymap::from_text(&text).unwrap();
            assert_eq!(back.name, km.name);
            assert_eq!(back.keys, km.keys);
            assert_eq!(back.compose, km.compose);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(Keymap::from_text("key 0x04 base char").is_err());
        assert!(Keymap::from_text("key 4 base char U+0061").is_err());
        assert!(Keymap::from_text("key 0x04 meio char U+0061").is_err());
        assert!(Keymap::from_text("compose U+D800 U+0061 U+0061").is_err());
        assert!(Keymap::from_text(&"\n".repeat(30_000)).is_err());
    }
}
