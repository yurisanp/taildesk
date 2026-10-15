//! Layouts de referência para testes, escritos à mão a partir do comportamento do SO.
//!
//! Na fase 2 estes dados passam a ser gerados por dump real (`ToUnicodeEx` no Windows,
//! `xkbcommon` no Linux) e gravados como arquivos; os testes continuam os mesmos.

use crate::{Hid, Keymap, Level, Output};

fn letters_and_space(keys: &mut Vec<(Hid, Level, Output)>) {
    for (i, c) in ('a'..='z').enumerate() {
        let hid = 0x04 + i as Hid;
        keys.push((hid, Level::Base, Output::Char(c)));
        keys.push((hid, Level::Shift, Output::Char(c.to_ascii_uppercase())));
    }
    keys.push((0x2C, Level::Base, Output::Char(' ')));
    keys.push((0x2C, Level::Shift, Output::Char(' ')));
}

fn digits(keys: &mut Vec<(Hid, Level, Output)>, shifted: [Output; 10]) {
    for (i, (c, s)) in "1234567890".chars().zip(shifted).enumerate() {
        let hid = 0x1E + i as Hid;
        keys.push((hid, Level::Base, Output::Char(c)));
        keys.push((hid, Level::Shift, s));
    }
}

fn pairs(keys: &mut Vec<(Hid, Level, Output)>, list: &[(Hid, Output, Output)]) {
    for &(hid, base, shift) in list {
        keys.push((hid, Level::Base, base));
        keys.push((hid, Level::Shift, shift));
    }
}

/// Teclado numérico com o Num Lock ligado. Com ele desligado as teclas viram navegação
/// (`keypad_nav`), o que não depende do layout.
fn keypad(keys: &mut Vec<(Hid, Level, Output)>, decimal: char) {
    for (hid, c) in [(0x54, '/'), (0x55, '*'), (0x56, '-'), (0x57, '+')] {
        keys.push((hid, Level::Base, Output::Char(c)));
    }
    for (i, c) in "1234567890".chars().enumerate() {
        keys.push((0x59 + i as Hid, Level::Base, Output::Char(c)));
    }
    keys.push((0x63, Level::Base, Output::Char(decimal)));
}

/// `(morta, bases, resultados)` -> entradas de composição, com maiúsculas e o acento sozinho.
fn compose(out: &mut Vec<(char, char, char)>, dead: char, bases: &str, results: &str) {
    for (b, r) in bases.chars().zip(results.chars()) {
        out.push((dead, b, r));
        let (bu, ru) = (
            b.to_uppercase().next().unwrap(),
            r.to_uppercase().next().unwrap(),
        );
        if bu != b {
            out.push((dead, bu, ru));
        }
    }
    out.push((dead, ' ', dead));
}

use Output::{Char as C, Dead as D};

/// Português (Brasil) ABNT2, com o comportamento do Windows (`´` + `c` = `ç`; no Linux o
/// Compose padrão dá `ć`, a diferença só importa quando o ABNT2 é o controlador).
pub fn abnt2() -> Keymap {
    let mut keys = Vec::new();
    letters_and_space(&mut keys);
    digits(
        &mut keys,
        [
            C('!'),
            C('@'),
            C('#'),
            C('$'),
            C('%'),
            D('¨'),
            C('&'),
            C('*'),
            C('('),
            C(')'),
        ],
    );
    pairs(
        &mut keys,
        &[
            (0x2D, C('-'), C('_')),
            (0x2E, C('='), C('+')),
            (0x2F, D('´'), D('`')),
            (0x30, C('['), C('{')),
            (0x32, C(']'), C('}')),
            (0x33, C('ç'), C('Ç')),
            (0x34, D('~'), D('^')),
            (0x35, C('\''), C('"')),
            (0x36, C(','), C('<')),
            (0x37, C('.'), C('>')),
            (0x38, C(';'), C(':')),
            (0x87, C('/'), C('?')),
            (0x64, C('\\'), C('|')),
        ],
    );
    for (hid, c) in [
        (0x14, '/'),
        (0x1A, '?'),
        (0x08, '°'),
        (0x1E, '¹'),
        (0x1F, '²'),
        (0x20, '³'),
    ] {
        keys.push((hid, Level::AltGr, C(c)));
    }
    keypad(&mut keys, ',');
    // Keypad Comma, a tecla extra do numérico do ABNT2: produz `.` (o `,` fica no decimal).
    keys.push((0x85, Level::Base, C('.')));
    let mut comp = Vec::new();
    compose(&mut comp, '´', "aeiouc", "áéíóúç");
    compose(&mut comp, '`', "aeiou", "àèìòù");
    compose(&mut comp, '~', "aon", "ãõñ");
    compose(&mut comp, '^', "aeiou", "âêîôû");
    compose(&mut comp, '¨', "aeiou", "äëïöü");
    Keymap {
        name: "br-abnt2".into(),
        keys,
        compose: comp,
    }
}

/// Inglês (EUA) Internacional, com o comportamento do Windows (`'` + `c` = `ç`).
pub fn us_intl_windows() -> Keymap {
    let mut keys = Vec::new();
    letters_and_space(&mut keys);
    digits(
        &mut keys,
        [
            C('!'),
            C('@'),
            C('#'),
            C('$'),
            C('%'),
            D('^'),
            C('&'),
            C('*'),
            C('('),
            C(')'),
        ],
    );
    pairs(
        &mut keys,
        &[
            (0x2D, C('-'), C('_')),
            (0x2E, C('='), C('+')),
            (0x2F, C('['), C('{')),
            (0x30, C(']'), C('}')),
            (0x31, C('\\'), C('|')),
            (0x33, C(';'), C(':')),
            (0x34, D('\''), D('"')),
            (0x35, D('`'), D('~')),
            (0x36, C(','), C('<')),
            (0x37, C('.'), C('>')),
            (0x38, C('/'), C('?')),
        ],
    );
    for (hid, c) in [
        (0x04, 'á'),
        (0x08, 'é'),
        (0x0C, 'í'),
        (0x12, 'ó'),
        (0x18, 'ú'),
        (0x36, 'ç'),
        (0x16, 'ß'),
        (0x11, 'ñ'),
    ] {
        keys.push((hid, Level::AltGr, C(c)));
    }
    keypad(&mut keys, '.');
    let mut comp = Vec::new();
    compose(&mut comp, '\'', "aeiouc", "áéíóúç");
    compose(&mut comp, '"', "aeiou", "äëïöü");
    compose(&mut comp, '`', "aeiou", "àèìòù");
    compose(&mut comp, '~', "aon", "ãõñ");
    compose(&mut comp, '^', "aeiou", "âêîôû");
    Keymap {
        name: "us-intl".into(),
        keys,
        compose: comp,
    }
}

/// Inglês (EUA) sem teclas mortas nem AltGr.
pub fn us() -> Keymap {
    let mut keys = Vec::new();
    letters_and_space(&mut keys);
    digits(
        &mut keys,
        [
            C('!'),
            C('@'),
            C('#'),
            C('$'),
            C('%'),
            C('^'),
            C('&'),
            C('*'),
            C('('),
            C(')'),
        ],
    );
    pairs(
        &mut keys,
        &[
            (0x2D, C('-'), C('_')),
            (0x2E, C('='), C('+')),
            (0x2F, C('['), C('{')),
            (0x30, C(']'), C('}')),
            (0x31, C('\\'), C('|')),
            (0x33, C(';'), C(':')),
            (0x34, C('\''), C('"')),
            (0x35, C('`'), C('~')),
            (0x36, C(','), C('<')),
            (0x37, C('.'), C('>')),
            (0x38, C('/'), C('?')),
        ],
    );
    keypad(&mut keys, '.');
    Keymap {
        name: "us".into(),
        keys,
        compose: Vec::new(),
    }
}
