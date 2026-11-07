//! Configurações que sobrevivem ao app: "Este computador" (dispositivos lembrados, pedir
//! sempre) e os fingerprints dos hosts já vistos (TOFU). Nada aqui é segredo: a chave da
//! identidade fica no cofre do sistema.
//!
//! Arquivo de texto, uma linha por item, campos separados por tabulação:
//! `pedir_sempre\t1`, `confiavel\t<node_stable_id>\t<nome>`, `host\t<node_stable_id>\t<sha256>`,
//! `atualizar_auto\t1`, `canal\testavel|pre`, `codec\tauto|h264|hevc|av1|pyrowave`,
//! `taxa_kbps\t<n>` (0 = automática), `senha_hash\t<argon2 PHC>` ("Acesso com senha": só o
//! hash, nunca a senha), `monitores\tabas|janelas|todos\t<seguir 0|1>` e
//! `monitores_host\t<node_stable_id>\t<modo>\t<seguir>` (o último usado com aquele computador),
//! `celulares\t0|1` ("Permitir celulares") e
//! `celular\t<node_stable_id>\t<nome>\t<aparelho>\t<ms>` (celular pareado por QR) e
//! `placa\t<node_stable_id>\t<mac>\t<ipv4>/<prefixo>\t<mac do gateway ou ->\t<cabo 0|1>\t<wol>`
//! (placas de rede de um host, para o Wake on LAN; `wol` = 0 não sabe, 1 armada, 2 desligada).

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use taildesk_proto::control::{Codec, LanIface, limits};
use taildesk_session::{HostSettings, PairedPhone, TrustedDevice};
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
    /// Placas de rede de cada host (`node_stable_id`), do último `PowerInfo` (Wake on LAN).
    pub energia: BTreeMap<String, Vec<LanIface>>,
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
            ["celulares", v] => c.host.allow_phones = *v != "0",
            ["celular", id, nome, aparelho, ms] if !id.is_empty() => {
                c.host.phones.push(PairedPhone {
                    node_stable_id: (*id).to_owned(),
                    name: (*nome).to_owned(),
                    device_name: (*aparelho).to_owned(),
                    paired_at_ms: ms.parse().unwrap_or(0),
                });
            }
            ["placa", id, mac, rede, gw, cabo, wol] if !id.is_empty() => {
                if let Some(p) = placa_de(mac, rede, gw, cabo, wol) {
                    let lista = c.energia.entry((*id).to_owned()).or_default();
                    if lista.len() < limits::LAN_IFACES {
                        lista.push(p);
                    }
                }
            }
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
    s += &format!("celulares\t{}\n", u8::from(c.host.allow_phones));
    for p in &c.host.phones {
        s += &format!(
            "celular\t{}\t{}\t{}\t{}\n",
            limpo(&p.node_stable_id),
            limpo(&p.name),
            limpo(&p.device_name),
            p.paired_at_ms
        );
    }
    for (id, fp) in &c.hosts {
        s += &format!("host\t{}\t{}\n", limpo(id), to_hex(fp));
    }
    for (id, placas) in &c.energia {
        for p in placas.iter().take(limits::LAN_IFACES) {
            let Some(mac) = mac_texto(&p.mac) else {
                continue;
            };
            s += &format!(
                "placa\t{}\t{mac}\t{}/{}\t{}\t{}\t{}\n",
                limpo(id),
                std::net::Ipv4Addr::from(p.ipv4),
                p.prefix_len.min(32),
                mac_texto(&p.gateway_mac).unwrap_or_else(|| "-".into()),
                u8::from(p.wired),
                p.wol.clamp(0, 2)
            );
        }
    }
    s
}

/// `aa:bb:cc:dd:ee:ff`; `None` se não tiver 6 bytes.
fn mac_texto(mac: &[u8]) -> Option<String> {
    let mac: [u8; 6] = mac.try_into().ok()?;
    Some(taildesk_session::acordar::mac_texto(mac))
}

fn mac_de_texto(s: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = s
        .split(':')
        .map(|b| u8::from_str_radix(b, 16).ok().filter(|_| b.len() == 2))
        .collect::<Option<_>>()?;
    (bytes.len() == 6).then_some(bytes)
}

fn placa_de(mac: &str, rede: &str, gw: &str, cabo: &str, wol: &str) -> Option<LanIface> {
    let (ip, prefixo) = rede.split_once('/')?;
    let ip: std::net::Ipv4Addr = ip.parse().ok()?;
    let prefix_len: u32 = prefixo.parse().ok().filter(|p| *p <= 32)?;
    Some(LanIface {
        mac: mac_de_texto(mac)?,
        ipv4: u32::from(ip),
        prefix_len,
        gateway_mac: if gw == "-" {
            Vec::new()
        } else {
            mac_de_texto(gw)?
        },
        wired: cabo == "1",
        wol: wol.parse::<i32>().ok().filter(|w| (0..=2).contains(w))?,
    })
}

/// Lê um arquivo de configuração em outro lugar (o agente do serviço do Windows lê o do último
/// usuário); sem arquivo ou ilegível, o padrão.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn ler_de(caminho: &std::path::Path) -> Config {
    std::fs::read_to_string(caminho)
        .map(|t| analisar(&t))
        .unwrap_or_default()
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
        c.host.allow_phones = false;
        c.host.phones.push(PairedPhone {
            node_stable_id: "nCEL".into(),
            name: "pixel-8".into(),
            device_name: "Pixel\tdo Rafael".into(),
            paired_at_ms: 1_762_000_000_000,
        });
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
        assert!(!volta.host.allow_phones);
        assert_eq!(volta.host.phones.len(), 1);
        assert_eq!(volta.host.phones[0].device_name, "Pixeldo Rafael");
        assert_eq!(volta.host.phones[0].paired_at_ms, 1_762_000_000_000);
        assert!(analisar("").host.allow_phones);
        assert_eq!(analisar("senha_hash\tqualquer\n").host.password_hash, None);
        // Linhas desconhecidas ou estragadas não derrubam a leitura.
        let estragado = format!("{texto}futuro\tx\nhost\tnQ\tzz\nconfiavel\t\tsem id\n");
        let v = analisar(&estragado);
        assert_eq!(v.hosts.len(), 1);
        assert_eq!(v.host.trusted.len(), 1);
    }

    #[test]
    fn placas_de_rede_ida_e_volta() {
        let mut c = Config::default();
        let placas = vec![
            LanIface {
                mac: vec![0x3c, 0x7c, 0x3f, 1, 0xab, 3],
                ipv4: u32::from(std::net::Ipv4Addr::new(192, 168, 0, 20)),
                prefix_len: 24,
                gateway_mac: vec![0xa4, 0x2b, 0xb0, 9, 8, 7],
                wired: true,
                wol: 1,
            },
            LanIface {
                mac: vec![2, 0, 0, 0, 0, 1],
                ipv4: u32::from(std::net::Ipv4Addr::new(10, 0, 0, 5)),
                prefix_len: 8,
                gateway_mac: Vec::new(),
                wired: false,
                wol: 0,
            },
        ];
        c.energia.insert("nPC".into(), placas.clone());
        let texto = escrever(&c);
        assert!(
            texto.contains(
                "placa\tnPC\t3c:7c:3f:01:ab:03\t192.168.0.20/24\ta4:2b:b0:09:08:07\t1\t1\n"
            )
        );
        assert_eq!(analisar(&texto).energia.get("nPC"), Some(&placas));
        // Estragadas: MAC curto, prefixo > 32, IP inválido, wol fora da faixa.
        for linha in [
            "placa\tnX\t3c:7c:3f:01:ab\t192.168.0.20/24\t-\t1\t0",
            "placa\tnX\t3c:7c:3f:01:ab:03\t192.168.0.20/33\t-\t1\t0",
            "placa\tnX\t3c:7c:3f:01:ab:03\t192.168.0/24\t-\t1\t0",
            "placa\tnX\t3c:7c:3f:01:ab:03\t192.168.0.20/24\t-\t1\t7",
            "placa\tnX\t3c:7c:3f:1:ab:03\t192.168.0.20/24\t-\t1\t0",
        ] {
            assert!(analisar(linha).energia.is_empty(), "{linha}");
        }
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
