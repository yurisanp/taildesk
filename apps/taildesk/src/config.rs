//! Configurações que sobrevivem ao app: "Este computador" (dispositivos lembrados, pedir
//! sempre) e os fingerprints dos hosts já vistos (TOFU). Nada aqui é segredo: a chave da
//! identidade fica no cofre do sistema.
//!
//! Arquivo de texto, uma linha por item, campos separados por tabulação:
//! `pedir_sempre\t1`, `confiavel\t<node_stable_id>\t<nome>`, `host\t<node_stable_id>\t<sha256>`,
//! `atualizar_auto\t1`, `canal\testavel|pre`, `codec\tauto|h264|hevc|av1|pyrowave`,
//! `taxa_kbps\t<n>` (0 = automática).

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use taildesk_proto::control::Codec;
use taildesk_session::{HostSettings, TrustedDevice};
use taildesk_transport::tls::{Fingerprint, from_hex, to_hex};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub host: HostSettings,
    /// Hosts conhecidos: `node_stable_id` → fingerprint do certificado.
    pub hosts: BTreeMap<String, Fingerprint>,
    pub atualizacao: PrefsAtualizacao,
    pub video: PrefsVideo,
}

/// Configurações → Sessão: vídeo pedido ao host em toda sessão nova (o seletor da janela da
/// sessão muda só a sessão aberta).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrefsVideo {
    /// `Unspecified` = automático.
    pub codec: Codec,
    /// `0` = automática.
    pub taxa_kbps: u32,
}

/// Nome do codec no arquivo.
fn nome_codec(c: Codec) -> &'static str {
    match c {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
        Codec::Av1 => "av1",
        Codec::Vp8 => "vp8",
        Codec::Vp9 => "vp9",
        Codec::PyroWave => "pyrowave",
        Codec::Unspecified => "auto",
    }
}

fn codec_de(nome: &str) -> Codec {
    [
        Codec::H264,
        Codec::Hevc,
        Codec::Av1,
        Codec::Vp8,
        Codec::Vp9,
        Codec::PyroWave,
    ]
    .into_iter()
    .find(|c| nome_codec(*c) == nome)
    .unwrap_or(Codec::Unspecified)
}

/// Configurações → Atualizações (`docs/context/16-atualizacao-automatica.md` §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefsAtualizacao {
    /// Procurar ao abrir e a cada 24 h (ligado por padrão).
    pub automatica: bool,
    /// Canal pré-lançamento (inclui as betas).
    pub pre: bool,
}

impl Default for PrefsAtualizacao {
    fn default() -> Self {
        PrefsAtualizacao {
            automatica: true,
            pre: false,
        }
    }
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
            ["atualizar_auto", v] => c.atualizacao.automatica = *v != "0",
            ["canal", v] => c.atualizacao.pre = *v == "pre",
            ["codec", v] => c.video.codec = codec_de(v),
            ["taxa_kbps", v] => {
                c.video.taxa_kbps = v
                    .parse::<u32>()
                    .unwrap_or(0)
                    .min(taildesk_proto::control::limits::VIDEO_KBPS);
            }
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
    s += &format!(
        "atualizar_auto\t{}\ncanal\t{}\n",
        u8::from(c.atualizacao.automatica),
        if c.atualizacao.pre { "pre" } else { "estavel" }
    );
    s += &format!(
        "codec\t{}\ntaxa_kbps\t{}\n",
        nome_codec(c.video.codec),
        c.video.taxa_kbps
    );
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
        c.atualizacao = PrefsAtualizacao {
            automatica: false,
            pre: true,
        };
        c.video = PrefsVideo {
            codec: Codec::Hevc,
            taxa_kbps: 20_000,
        };
        let texto = escrever(&c);
        let volta = analisar(&texto);
        assert!(!volta.host.ask_always);
        assert_eq!(volta.atualizacao, c.atualizacao);
        assert_eq!(volta.video, c.video);
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
        assert_eq!(analisar("").atualizacao, PrefsAtualizacao::default());
        assert_eq!(analisar("").video, PrefsVideo::default());
        // Codec desconhecido (de uma versão futura) e taxa absurda voltam ao automático/teto.
        let v = analisar("codec\tvvc\ntaxa_kbps\t99999999999\n").video;
        assert_eq!(v.codec, Codec::Unspecified);
        assert_eq!(v.taxa_kbps, 0);
    }
}
