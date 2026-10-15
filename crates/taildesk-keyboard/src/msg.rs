//! Mensagens de teclado no fio (`11-teclado.md` §3), sem depender do protobuf.
//!
//! `taildesk-proto` converte estes tipos de e para as mensagens `Key`, `KeyShortcut`, `KeyChar`
//! e `ReleaseAll`.

use crate::{Hid, Mods};

/// Estado dos locks do teclado.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Locks {
    pub caps: bool,
    pub num: bool,
}

impl Locks {
    /// Formato do campo `locks` de `Key`: bit 0 = Caps, bit 1 = Num.
    pub fn bits(self) -> u32 {
        u32::from(self.caps) | u32::from(self.num) << 1
    }

    pub fn from_bits(bits: u32) -> Locks {
        Locks {
            caps: bits & 1 != 0,
            num: bits & 2 != 0,
        }
    }
}

/// O que o controlador manda para o host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyMsg {
    /// Tecla física. `locks` vai só no modo Físico, para o host sincronizar Caps e Num Lock.
    Key {
        hid: Hid,
        down: bool,
        locks: Option<Locks>,
    },
    /// Atalho pela letra: `base_char` é o que a tecla produz sem modificador no controlador.
    Shortcut {
        base_char: char,
        hid_hint: Hid,
        mods: Mods,
        down: bool,
    },
    /// Texto já resolvido (teclas mortas compostas no controlador).
    Char { text: String, hid_hint: Hid },
    /// Soltar tudo que estiver apertado no host (perda de foco).
    ReleaseAll,
}

/// Escolha do usuário por host (`KeyboardMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Auto,
    Physical,
    Text,
}

/// Como o controlador traduz de fato.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Translation {
    /// Toda tecla vai pela posição física.
    Physical,
    /// Auto híbrido de `11-teclado.md` §2.3: física, atalho pela letra ou caractere.
    Hybrid,
}

/// Decide a tradução a partir da escolha do usuário e dos `layout_tag` dos dois lados.
///
/// Em Auto, layouts iguais vão por posição física (mais fiel) e diferentes pelo híbrido.
/// Texto força o híbrido mesmo com layouts iguais. Tag vazia conta como desconhecida
/// (diferente), porque errar para o híbrido ainda digita certo.
pub fn effective_translation(mode: Mode, local_tag: &str, remote_tag: &str) -> Translation {
    match mode {
        Mode::Physical => Translation::Physical,
        Mode::Text => Translation::Hybrid,
        Mode::Auto if !local_tag.is_empty() && local_tag == remote_tag => Translation::Physical,
        Mode::Auto => Translation::Hybrid,
    }
}

/// `layout_tag` normalizado a partir do id do SO (`KeyboardInfo.os_layout_id`).
///
/// Só layouts que digitam igual ganham a mesma tag; o resto passa sem mudar e conta como
/// diferente, o que no máximo troca Físico pelo híbrido (que sempre digita certo).
/// Por isso as teclas mortas separam Windows e Linux: no ABNT2 do Linux `´` + espaço dá `'`
/// (no Windows dá `´`) e no US-Intl do Linux `'` + `c` dá `ć` (no Windows, `ç`).
pub fn layout_tag(os_layout_id: &str) -> String {
    let id = os_layout_id.to_ascii_lowercase();
    match id.as_str() {
        "00000409" | "xkb:us" => "us",
        "00020409" => "us-intl",
        "xkb:us(intl)" => "us-intl-xkb",
        "00010416" => "br-abnt2",
        "xkb:br" | "xkb:br(abnt2)" => "br-abnt2-xkb",
        "00000416" => "br-abnt",
        _ => return os_layout_id.to_string(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_round_trip() {
        for bits in 0..4 {
            assert_eq!(Locks::from_bits(bits).bits(), bits);
        }
    }

    #[test]
    fn tags() {
        assert_eq!(layout_tag("00010416"), "br-abnt2");
        assert_eq!(layout_tag("xkb:br(abnt2)"), "br-abnt2-xkb");
        assert_eq!(layout_tag("xkb:br"), "br-abnt2-xkb");
        assert_ne!(layout_tag("xkb:br(abnt2)"), layout_tag("00010416"));
        assert_eq!(layout_tag("00020409"), "us-intl");
        assert_ne!(layout_tag("xkb:us(intl)"), layout_tag("00020409"));
        assert_eq!(layout_tag("xkb:fr(azerty)"), "xkb:fr(azerty)");
    }

    #[test]
    fn translation_choice() {
        use Translation::*;
        assert_eq!(
            effective_translation(Mode::Auto, "br-abnt2", "br-abnt2"),
            Physical
        );
        assert_eq!(
            effective_translation(Mode::Auto, "us-intl", "br-abnt2"),
            Hybrid
        );
        assert_eq!(effective_translation(Mode::Auto, "", ""), Hybrid);
        assert_eq!(
            effective_translation(Mode::Text, "br-abnt2", "br-abnt2"),
            Hybrid
        );
        assert_eq!(
            effective_translation(Mode::Physical, "us", "br-abnt2"),
            Physical
        );
    }
}
