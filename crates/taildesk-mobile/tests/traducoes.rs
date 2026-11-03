//! Toda frase `@tr("…")` das telas tem tradução em inglês (regra do `AGENTS.md`: textos de UI em
//! pt-BR e en). Falta de tradução quebra o build, como o lint de strings do babylock.

use std::collections::BTreeSet;
use std::path::Path;

fn frases_das_telas() -> BTreeSet<String> {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut frases = BTreeSet::new();
    for arq in std::fs::read_dir(ui).unwrap() {
        let caminho = arq.unwrap().path();
        if caminho.extension().is_none_or(|e| e != "slint") {
            continue;
        }
        let texto = std::fs::read_to_string(&caminho).unwrap();
        for pedaco in texto.split("@tr(\"").skip(1) {
            let mut frase = String::new();
            let mut escape = false;
            for c in pedaco.chars() {
                match (escape, c) {
                    (true, c) => {
                        frase.push(c);
                        escape = false;
                    }
                    (false, '\\') => escape = true,
                    (false, '"') => break,
                    (false, c) => frase.push(c),
                }
            }
            frases.insert(frase);
        }
    }
    frases
}

fn traduzidas() -> BTreeSet<String> {
    let po =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("traducoes/en/LC_MESSAGES/taildesk-mobile.po");
    let texto = std::fs::read_to_string(po).unwrap();
    let mut ids = BTreeSet::new();
    let mut linhas = texto.lines().peekable();
    while let Some(l) = linhas.next() {
        let Some(id) = l.strip_prefix("msgid \"").and_then(|r| r.strip_suffix('"')) else {
            continue;
        };
        let traducao = linhas
            .peek()
            .and_then(|m| m.strip_prefix("msgstr \""))
            .unwrap_or("\"");
        if !id.is_empty() && traducao != "\"" {
            ids.insert(id.replace("\\\"", "\"").replace("\\\\", "\\"));
        }
    }
    ids
}

#[test]
fn traducoes_completas() {
    let faltam: Vec<String> = frases_das_telas()
        .difference(&traduzidas())
        .cloned()
        .collect();
    assert!(faltam.is_empty(), "sem tradução em inglês: {faltam:#?}");
}
