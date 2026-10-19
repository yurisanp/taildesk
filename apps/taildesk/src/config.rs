//! Configurações que sobrevivem ao app: "Este computador" (dispositivos lembrados, pedir
//! sempre) e os fingerprints dos hosts já vistos (TOFU). Nada aqui é segredo: a chave da
//! identidade fica no cofre do sistema.
//!
//! Arquivo de texto, uma linha por item, campos separados por tabulação:
//! `pedir_sempre\t1`, `confiavel\t<node_stable_id>\t<nome>`, `host\t<node_stable_id>\t<sha256>`,
//! `atualizar_auto\t1`, `canal\testavel|pre`, `codec\tauto|h264|hevc|av1|pyrowave`,
//! `taxa_kbps\t<n>` (0 = automática), `senha_hash\t<argon2 PHC>` ("Acesso com senha": só o
//! hash, nunca a senha), `monitores\tabas|janelas|todos\t<seguir 0|1>` e
//! `monitores_host\t<node_stable_id>\t<modo>\t<seguir>` (o último usado com aquele computador).

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
    /// Configurações → Sessão: como abrir os monitores do host.
    pub monitores: PrefsMonitores,
    /// O que foi usado por último com cada host (`node_stable_id`), por cima do padrão.
    pub monitores_host: BTreeMap<String, PrefsMonitores>,
}

/// Como a sessão mostra os monitores do host ao conectar (`13-multimonitor.md` §2.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModoMonitores {
    /// Um por vez, em abas.
    #[default]
    Abas,
    /// Cada monitor do host numa janela própria.
    Janelas,
    /// "Usar todos os meus monitores": cada um em tela cheia num monitor local.
    Todos,
}

impl ModoMonitores {
    /// Na ordem da lista das Configurações.
    pub const TODOS: [ModoMonitores; 3] = [
        ModoMonitores::Abas,
        ModoMonitores::Janelas,
        ModoMonitores::Todos,
    ];

    fn nome(self) -> &'static str {
        match self {
            ModoMonitores::Abas => "abas",
            ModoMonitores::Janelas => "janelas",
            ModoMonitores::Todos => "todos",
        }
    }

    fn de(nome: &str) -> ModoMonitores {
        Self::TODOS
            .into_iter()
            .find(|m| m.nome() == nome)
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrefsMonitores {
    pub modo: ModoMonitores,
    pub seguir_mouse: bool,
}

impl Config {
    /// Preferência de monitores para o host `node` (a lembrada, ou o padrão).
    pub fn monitores_de(&self, node: Option<&str>) -> PrefsMonitores {
        node.and_then(|n| self.monitores_host.get(n))
            .copied()
            .unwrap_or(self.monitores)
    }
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
            ["monitores", modo, seguir] => {
                c.monitores = PrefsMonitores {
                    modo: ModoMonitores::de(modo),
                    seguir_mouse: *seguir == "1",
                };
            }
            ["monitores_host", id, modo, seguir] if !id.is_empty() => {
                c.monitores_host.insert(
                    (*id).to_owned(),
                    PrefsMonitores {
                        modo: ModoMonitores::de(modo),
                        seguir_mouse: *seguir == "1",
                    },
                );
            }
            ["senha_hash", h] if h.starts_with("$argon2") => {
                c.host.password_hash = Some((*h).to_owned());
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
    s += &format!(
        "monitores\t{}\t{}\n",
        c.monitores.modo.nome(),
        u8::from(c.monitores.seguir_mouse)
    );
    for (id, m) in &c.monitores_host {
        s += &format!(
            "monitores_host\t{}\t{}\t{}\n",
            limpo(id),
            m.modo.nome(),
            u8::from(m.seguir_mouse)
        );
    }
    if let Some(h) = &c.host.password_hash {
        s += &format!("senha_hash\t{}\n", limpo(h));
    }
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
    // O hash da senha de acesso fica aqui: só o dono lê.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
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
        c.host.password_hash = Some("$argon2id$v=19$m=19456,t=2,p=1$c2Fs$aGFzaA".into());
        c.atualizacao = PrefsAtualizacao {
            automatica: false,
            pre: true,
        };
        c.video = PrefsVideo {
            codec: Codec::Hevc,
            taxa_kbps: 20_000,
        };
        c.monitores = PrefsMonitores {
            modo: ModoMonitores::Janelas,
            seguir_mouse: true,
        };
        c.monitores_host.insert(
            "nXYZ".into(),
            PrefsMonitores {
                modo: ModoMonitores::Todos,
                seguir_mouse: false,
            },
        );
        let texto = escrever(&c);
        let volta = analisar(&texto);
        assert_eq!(volta.monitores, c.monitores);
        assert_eq!(volta.monitores_host, c.monitores_host);
        assert_eq!(volta.monitores_de(Some("nXYZ")).modo, ModoMonitores::Todos);
        assert_eq!(volta.monitores_de(Some("outro")), c.monitores);
        assert_eq!(volta.monitores_de(None), c.monitores);
        assert!(!volta.host.ask_always);
        assert_eq!(volta.atualizacao, c.atualizacao);
        assert_eq!(volta.video, c.video);
        assert_eq!(volta.host.trusted[0].name, "notebookyuri");
        assert_eq!(volta.hosts, c.hosts);
        assert_eq!(volta.host.password_hash, c.host.password_hash);
        assert_eq!(analisar("senha_hash\tqualquer\n").host.password_hash, None);
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
        assert_eq!(analisar("").monitores.modo, ModoMonitores::Abas);
        assert!(!analisar("").monitores.seguir_mouse);
        // Modo de uma versão futura cai em abas.
        assert_eq!(
            analisar("monitores\tmosaico\t1\n").monitores,
            PrefsMonitores {
                modo: ModoMonitores::Abas,
                seguir_mouse: true
            }
        );
        // Codec desconhecido (de uma versão futura) e taxa absurda voltam ao automático/teto.
        let v = analisar("codec\tvvc\ntaxa_kbps\t99999999999\n").video;
        assert_eq!(v.codec, Codec::Unspecified);
        assert_eq!(v.taxa_kbps, 0);
    }
}
