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
