//! Textos montados no Rust (HUD, etapas da conexão, erros), em pt-BR e en (regra do
//! `AGENTS.md`; MF-03). Os textos fixos das telas ficam no Slint com `@tr`.

/// Idioma da interface, do sistema (`Locale.getDefault()`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Idioma {
    #[default]
    Pt,
    En,
}

impl Idioma {
    /// "pt-BR", "pt_PT", "en-US"… Qualquer português vira pt; o resto, en.
    pub fn do_sistema(tag: &str) -> Idioma {
        if tag.to_ascii_lowercase().starts_with("pt") {
            Idioma::Pt
        } else {
            Idioma::En
        }
    }

    /// Escolhe o texto do idioma.
    pub fn t<'a>(self, pt: &'a str, en: &'a str) -> &'a str {
        match self {
            Idioma::Pt => pt,
            Idioma::En => en,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idioma_do_sistema() {
        assert_eq!(Idioma::do_sistema("pt-BR"), Idioma::Pt);
        assert_eq!(Idioma::do_sistema("PT_pt"), Idioma::Pt);
        assert_eq!(Idioma::do_sistema("en-US"), Idioma::En);
        assert_eq!(Idioma::do_sistema("es-ES"), Idioma::En);
        assert_eq!(Idioma::En.t("olá", "hi"), "hi");
    }
}
