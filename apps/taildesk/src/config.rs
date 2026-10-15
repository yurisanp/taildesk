//! Configurações que sobrevivem ao app: "Este computador" (dispositivos lembrados, pedir
//! sempre) e os fingerprints dos hosts já vistos (TOFU). Nada aqui é segredo: a chave da
//! identidade fica no cofre do sistema.
//!
//! Arquivo de texto, uma linha por item, campos separados por tabulação:
//! `pedir_sempre\t1`, `confiavel\t<node_stable_id>\t<nome>`, `host\t<node_stable_id>\t<sha256>`.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use taildesk_session::{HostSettings, TrustedDevice};
use taildesk_transport::tls::{Fingerprint, from_hex, to_hex};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub host: HostSettings,
    /// Hosts conhecidos: `node_stable_id` → fingerprint do certificado.
    pub hosts: BTreeMap<String, Fingerprint>,
}

fn caminho() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("TailDesk"));
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("taildesk"));
    base.map(|d| d.join("config"))
}

/// Sem tabulação nem quebra de linha, para não quebrar o formato.
fn limpo(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

fn analisar(texto: &str) -> Config {
    let mut c = Config::default();
    for linha in texto.lines() {
        let campos: Vec<&str> = linha.split('\t').collect();
        match campos.as_slice() {
            ["pedir_sempre", v] => c.host.ask_always = *v != "0",
            ["confiavel", id, nome] if !id.is_empty() => c.host.trusted.push(TrustedDevice {
                node_stable_id: (*id).to_owned(),
                name: (*nome).to_owned(),
            }),
            ["host", id, fp] if !id.is_empty() => {
                if let Some(fp) = from_hex(fp) {
                    c.hosts.insert((*id).to_owned(), fp);
                }
            }
            // Linha de uma versão mais nova ou estragada: ignora.
            _ => {}
        }
    }
    c
}

fn escrever(c: &Config) -> String {
    let mut s = format!("pedir_sempre\t{}\n", u8::from(c.host.ask_always));
    for t in &c.host.trusted {
        s += &format!(
            "confiavel\t{}\t{}\n",
            limpo(&t.node_stable_id),
            limpo(&t.name)
        );
    }
    for (id, fp) in &c.hosts {
        s += &format!("host\t{}\t{}\n", limpo(id), to_hex(fp));
    }
    s
}

/// Lê o arquivo; sem arquivo (primeira vez) ou ilegível, o padrão.
pub fn ler() -> Config {
    caminho()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| analisar(&t))
        .unwrap_or_default()
}

/// Grava num temporário e renomeia, para nunca deixar o arquivo pela metade.
pub fn gravar(c: &Config) -> io::Result<()> {
    let p = caminho().ok_or_else(|| io::Error::other("sem pasta de configuração"))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, escrever(c))?;
    std::fs::rename(tmp, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_e_volta() {
        let mut c = Config::default();
        c.host.ask_always = false;
        c.host.trusted.push(TrustedDevice {
            node_stable_id: "nABC".into(),
            name: "notebook\tyuri\n".into(),
        });
        c.hosts.insert("nXYZ".into(), [7; 32]);
        let texto = escrever(&c);
        let volta = analisar(&texto);
        assert!(!volta.host.ask_always);
        assert_eq!(volta.host.trusted[0].name, "notebookyuri");
        assert_eq!(volta.hosts, c.hosts);
        // Linhas desconhecidas ou estragadas não derrubam a leitura.
        let estragado = format!("{texto}futuro\tx\nhost\tnQ\tzz\nconfiavel\t\tsem id\n");
        let v = analisar(&estragado);
        assert_eq!(v.hosts.len(), 1);
        assert_eq!(v.host.trusted.len(), 1);
    }

    #[test]
    fn padrao_pede_sempre() {
        assert!(analisar("").host.ask_always);
    }
}
