//! Teclado físico (USB ou Bluetooth) ligado ao celular durante a sessão (M3).
//!
//! O Android entrega a tecla à casca Kotlin (`TecladoFisico.kt`) depois do Slint, que só fica
//! com Tab e Esc (a navegação de foco dele come as duas); a casca já aplica o layout do aparelho,
//! resolve as teclas mortas e manda uma linha `tipo\tvalor\tmods` pela ponte:
//! - `tecla\t<nome>\t<mods>`: tecla sem texto, com o nome da faixa (`esc`, `f5`, `esquerda`…);
//! - `texto\t<texto>\t<mods>`: o texto que a tecla produziu.
//!
//! `<mods>` tem uma letra por modificador apertado: `c` Ctrl, `a` Alt, `s` Shift, `m` Meta.

use taildesk_keyboard::Mods;
use taildesk_keyboard::virtual_kb::TeclaFaixa;

/// Maior texto de uma tecla (uma tecla morta mal resolvida dá dois caracteres).
const LIMITE: usize = 16;

/// Uma tecla lida da ponte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lida {
    Tecla(TeclaFaixa, Mods),
    Texto(String, Mods),
}

/// Tecla pelo nome da faixa de teclas especiais (o mesmo nome do Slint e da casca Kotlin).
pub fn tecla_pelo_nome(nome: &str) -> Option<TeclaFaixa> {
    Some(match nome {
        "esc" => TeclaFaixa::Esc,
        "tab" => TeclaFaixa::Tab,
        "enter" => TeclaFaixa::Enter,
        "backspace" => TeclaFaixa::Backspace,
        "del" => TeclaFaixa::Delete,
        "insert" => TeclaFaixa::Insert,
        "esquerda" => TeclaFaixa::Esquerda,
        "direita" => TeclaFaixa::Direita,
        "cima" => TeclaFaixa::Cima,
        "baixo" => TeclaFaixa::Baixo,
        "home" => TeclaFaixa::Home,
        "end" => TeclaFaixa::End,
        "pgup" => TeclaFaixa::PageUp,
        "pgdn" => TeclaFaixa::PageDown,
        f if f.starts_with('f') => match f[1..].parse() {
            Ok(n @ 1..=12) => TeclaFaixa::F(n),
            _ => return None,
        },
        _ => return None,
    })
}

/// Modificadores pelas letras `c`, `a`, `s`, `m`.
pub fn mods(letras: &str) -> Mods {
    letras.chars().fold(Mods::NONE, |m, c| match c {
        'c' => m.with(Mods::CTRL),
        'a' => m.with(Mods::ALT),
        's' => m.with(Mods::SHIFT),
        'm' => m.with(Mods::META),
        _ => m,
    })
}

/// Lê uma linha da ponte; `None` se não é de teclado físico ou passou do limite.
pub fn ler(linha: &str) -> Option<Lida> {
    let mut campos = linha.splitn(3, '\t');
    let tipo = campos.next()?;
    let valor = campos.next()?;
    let m = mods(campos.next().unwrap_or(""));
    match tipo {
        "tecla" => Some(Lida::Tecla(tecla_pelo_nome(valor)?, m)),
        "texto" if !valor.is_empty() && valor.chars().count() <= LIMITE => {
            Some(Lida::Texto(valor.to_owned(), m))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_teclas_textos_e_modificadores() {
        assert_eq!(
            ler("tecla\tf5\t"),
            Some(Lida::Tecla(TeclaFaixa::F(5), Mods::NONE))
        );
        assert_eq!(
            ler("tecla\tesquerda\tcs"),
            Some(Lida::Tecla(
                TeclaFaixa::Esquerda,
                Mods::CTRL.with(Mods::SHIFT)
            ))
        );
        assert_eq!(ler("texto\tç\t"), Some(Lida::Texto("ç".into(), Mods::NONE)));
        // Tab no texto não quebra: só os dois primeiros separadores contam.
        assert_eq!(
            ler("texto\tc\tcam"),
            Some(Lida::Texto(
                "c".into(),
                Mods::CTRL.with(Mods::ALT).with(Mods::META)
            ))
        );
        assert_eq!(ler("tecla\tf13\t"), None);
        assert_eq!(ler("tecla\tvoar\t"), None);
        assert_eq!(ler("texto\t\t"), None);
        assert_eq!(ler(&format!("texto\t{}\t", "a".repeat(LIMITE + 1))), None);
        assert_eq!(ler("mouse\tx\t"), None);
        assert_eq!(ler(""), None);
    }

    #[test]
    fn nomes_da_faixa() {
        for n in [
            "esc",
            "tab",
            "enter",
            "backspace",
            "del",
            "insert",
            "esquerda",
            "direita",
            "cima",
            "baixo",
            "home",
            "end",
            "pgup",
            "pgdn",
            "f1",
            "f12",
        ] {
            assert!(tecla_pelo_nome(n).is_some(), "{n}");
        }
        assert_eq!(tecla_pelo_nome("f0"), None);
    }
}
