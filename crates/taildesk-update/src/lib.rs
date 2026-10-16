//! Atualização automática (`docs/context/16-atualizacao-automatica.md` §4).
//!
//! - [`procurar`] consulta as releases do GitHub e devolve a mais nova do canal, se for maior que
//!   a versão em execução.
//! - [`instalar_appimage`] baixa o AppImage dessa release ao lado do atual, confere o SHA-256 com o
//!   `SHA256SUMS` da mesma release e troca o arquivo com `rename`. O processo em execução mantém o
//!   inode antigo; a versão nova vale ao reiniciar ([`reiniciar`]).
//! - Fora do AppImage ([`Instalacao::detectar`]) o app só avisa, com link para a release.
//!
//! Sem telemetria: a única chamada é a API pública do GitHub, sem token. Tudo é bloqueante; o app
//! chama de uma thread própria.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const REPO: &str = "yurisanp/taildesk";

/// Canal de atualização (Configurações → Atualizações).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Canal {
    #[default]
    Estavel,
    /// Inclui as pre-releases (`v0.2.0-beta.1`).
    PreLancamento,
}

/// Como esta cópia do TailDesk foi instalada; decide se o app troca o arquivo ou só avisa.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instalacao {
    /// AppImage em `caminho` (variável `APPIMAGE` do runtime): o app se atualiza sozinho.
    AppImage { caminho: PathBuf },
    /// `.deb`, `.rpm` ou AUR em `/usr`: o gerenciador de pacotes atualiza.
    PacoteDistro,
    /// `.tar.gz` ou `.zip`: só aviso com link.
    Arquivo,
}

impl Instalacao {
    pub fn detectar() -> Self {
        if let Some(caminho) = std::env::var_os("APPIMAGE").filter(|c| !c.is_empty()) {
            return Instalacao::AppImage {
                caminho: PathBuf::from(caminho),
            };
        }
        let exe = std::env::current_exe().unwrap_or_default();
        if cfg!(target_os = "linux") && exe.starts_with("/usr") {
            Instalacao::PacoteDistro
        } else {
            Instalacao::Arquivo
        }
    }

    /// O app consegue se atualizar sozinho (AppImage numa pasta com escrita).
    pub fn automatica(&self) -> bool {
        match self {
            Instalacao::AppImage { caminho } => caminho.parent().is_some_and(pode_escrever),
            _ => false,
        }
    }
}

fn pode_escrever(pasta: &Path) -> bool {
    let teste = pasta.join(format!(".taildesk-teste-{}", std::process::id()));
    let ok = File::create(&teste).is_ok();
    let _ = fs::remove_file(&teste);
    ok
}

/// Uma release mais nova que a versão em execução.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Atualizacao {
    pub versao: Version,
    pub tag: String,
    /// Página da release (notas, download manual).
    pub pagina: String,
    pub notas: String,
    pub pre_lancamento: bool,
    appimage: Option<Asset>,
    sha256sums: Option<Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Deserialize)]
struct ReleaseApi {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// Erro da atualização. Nenhum deixa o app num estado pior: o `.part` é apagado e a versão atual
/// continua.
#[derive(Debug)]
pub enum Erro {
    Rede(String),
    /// Resposta da API ou `SHA256SUMS` fora do esperado.
    Formato(String),
    /// A release não tem AppImage ou não lista o hash dele.
    SemArquivo,
    /// O arquivo baixado não bate com o `SHA256SUMS`.
    HashDiferente,
    Io(io::Error),
}

impl fmt::Display for Erro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Erro::Rede(e) => write!(f, "falha de rede: {e}"),
            Erro::Formato(e) => write!(f, "resposta inesperada: {e}"),
            Erro::SemArquivo => f.write_str("a release não tem AppImage com SHA-256"),
            Erro::HashDiferente => f.write_str("o arquivo baixado não confere com o SHA256SUMS"),
            Erro::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Erro {}

impl From<io::Error> for Erro {
    fn from(e: io::Error) -> Self {
        Erro::Io(e)
    }
}

fn rede(e: ureq::Error) -> Erro {
    Erro::Rede(e.to_string())
}

fn agente() -> ureq::Agent {
    agente_com(10)
}

fn agente_com(redirecionamentos: u32) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .max_redirects(redirecionamentos)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(600)))
        .user_agent(concat!("TailDesk/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// GET de um asset de release, seguindo os redirecionamentos (github.com → CDN) um a um: cada
/// resposta 3xx é lida até o fim antes do próximo pedido. O redirecionamento automático do ureq
/// falhou ("chunk length cannot be read") com o 302 chunked do github.com atrás de um proxy.
fn baixar_url(agente: &ureq::Agent, url: &str) -> Result<ureq::http::Response<ureq::Body>, Erro> {
    let mut url = url.to_owned();
    for _ in 0..5 {
        let mut resp = agente.get(&url).call().map_err(rede)?;
        if !resp.status().is_redirection() {
            return Ok(resp);
        }
        let destino = resp
            .headers()
            .get("location")
            .and_then(|l| l.to_str().ok())
            .ok_or_else(|| Erro::Formato("redirecionamento sem Location".into()))?;
        if !destino.starts_with("https://") {
            return Err(Erro::Formato(format!("redirecionamento para {destino}")));
        }
        url = destino.to_owned();
        let _ = resp.body_mut().with_config().limit(1 << 16).read_to_vec();
    }
    Err(Erro::Rede("redirecionamentos demais".into()))
}

/// A versão em execução.
pub fn versao_atual() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// Procura uma release mais nova que `atual` no canal.
pub fn procurar(canal: Canal, atual: &Version) -> Result<Option<Atualizacao>, Erro> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=20");
    let releases: Vec<ReleaseApi> = agente()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(rede)?
        .body_mut()
        .read_json()
        .map_err(|e| Erro::Formato(e.to_string()))?;
    Ok(escolher(releases, canal, atual))
}

/// A release mais nova do canal acima de `atual`.
fn escolher(releases: Vec<ReleaseApi>, canal: Canal, atual: &Version) -> Option<Atualizacao> {
    releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let versao = Version::parse(r.tag_name.trim_start_matches('v')).ok()?;
            let pre = r.prerelease || !versao.pre.is_empty();
            if pre && canal == Canal::Estavel {
                return None;
            }
            let appimage = r
                .assets
                .iter()
                .find(|a| a.name.starts_with("TailDesk-") && a.name.ends_with("x86_64.AppImage"))
                .cloned();
            let sha256sums = r.assets.iter().find(|a| a.name == "SHA256SUMS").cloned();
            Some(Atualizacao {
                versao,
                tag: r.tag_name,
                pagina: r.html_url,
                notas: r.body.unwrap_or_default(),
                pre_lancamento: pre,
                appimage,
                sha256sums,
            })
        })
        .filter(|a| a.versao > *atual)
        .max_by(|a, b| a.versao.cmp(&b.versao))
}

/// Hash de `nome` num `SHA256SUMS` (`<hex>  <nome>` ou `<hex> *<nome>`, uma linha por arquivo).
fn hash_em_sums(sums: &str, nome: &str) -> Option<[u8; 32]> {
    sums.lines().find_map(|linha| {
        let (hex, resto) = linha.split_once(char::is_whitespace)?;
        let arquivo = resto.trim_start().trim_start_matches('*');
        if arquivo != nome || hex.len() != 64 {
            return None;
        }
        let mut h = [0u8; 32];
        for (i, b) in h.iter_mut().enumerate() {
            *b = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some(h)
    })
}

/// Baixa o AppImage de `atualizacao` para `"<appimage>.part"`, confere o SHA-256 e troca
/// `appimage` por ele. `progresso(baixados, total)` é chamado durante o download (`total` = 0
/// quando o tamanho não veio).
pub fn instalar_appimage(
    atualizacao: &Atualizacao,
    appimage: &Path,
    mut progresso: impl FnMut(u64, u64),
) -> Result<(), Erro> {
    let (Some(arquivo), Some(sums)) = (&atualizacao.appimage, &atualizacao.sha256sums) else {
        return Err(Erro::SemArquivo);
    };
    let agente = agente_com(0);
    let texto = baixar_url(&agente, &sums.browser_download_url)?
        .body_mut()
        .with_config()
        .limit(1 << 20)
        .read_to_string()
        .map_err(rede)?;
    let esperado = hash_em_sums(&texto, &arquivo.name).ok_or(Erro::SemArquivo)?;

    let mut nome_part = appimage.as_os_str().to_owned();
    nome_part.push(".part");
    let part = PathBuf::from(nome_part);
    let r = (|| {
        let mut resp = baixar_url(&agente, &arquivo.browser_download_url)?;
        let mut leitor = resp.body_mut().with_config().limit(1 << 30).reader();
        baixar(&mut leitor, &part, &esperado, arquivo.size, &mut progresso)?;
        trocar(&part, appimage)
    })();
    if r.is_err() {
        let _ = fs::remove_file(&part);
    }
    r
}

fn baixar(
    leitor: &mut impl Read,
    part: &Path,
    esperado: &[u8; 32],
    total: u64,
    progresso: &mut impl FnMut(u64, u64),
) -> Result<(), Erro> {
    let mut saida = File::create(part)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut baixados = 0u64;
    loop {
        let n = match leitor.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Erro::Rede(e.to_string())),
        };
        hash.update(&buf[..n]);
        saida.write_all(&buf[..n])?;
        baixados += n as u64;
        progresso(baixados, total);
    }
    saida.sync_all()?;
    if hash.finalize().as_slice() != esperado {
        return Err(Erro::HashDiferente);
    }
    Ok(())
}

fn trocar(part: &Path, appimage: &Path) -> Result<(), Erro> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(part, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(part, appimage)?;
    Ok(())
}

/// Troca o processo pelo AppImage novo (`exec`), com os mesmos argumentos. Só volta em erro.
#[cfg(unix)]
pub fn reiniciar(appimage: &Path) -> io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(appimage)
        .args(std::env::args_os().skip(1))
        .exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn releases() -> Vec<ReleaseApi> {
        serde_json::from_str(include_str!("../tests/releases.json")).unwrap()
    }

    #[test]
    fn escolhe_a_mais_nova_do_canal() {
        let atual = Version::new(0, 1, 0);
        let a = escolher(releases(), Canal::Estavel, &atual).unwrap();
        assert_eq!((a.tag.as_str(), a.pre_lancamento), ("v0.2.0", false));
        assert_eq!(a.appimage.unwrap().name, "TailDesk-0.2.0-x86_64.AppImage");
        assert!(a.sha256sums.is_some());
        let p = escolher(releases(), Canal::PreLancamento, &atual).unwrap();
        assert_eq!((p.tag.as_str(), p.pre_lancamento), ("v0.3.0-beta.1", true));
    }

    #[test]
    fn nada_quando_ja_esta_na_mais_nova() {
        let atual = Version::new(0, 2, 0);
        assert_eq!(escolher(releases(), Canal::Estavel, &atual), None);
        // Rascunho (v9.9.9) nunca entra.
        let p = escolher(releases(), Canal::PreLancamento, &atual).unwrap();
        assert_eq!(p.versao, Version::parse("0.3.0-beta.1").unwrap());
        assert_eq!(
            escolher(
                releases(),
                Canal::PreLancamento,
                &Version::parse("0.3.0").unwrap()
            ),
            None
        );
    }

    #[test]
    fn le_o_sha256sums() {
        let h = "ab".repeat(32);
        let sums = format!(
            "{}  TailDesk-0.2.0-x86_64.AppImage.zsync\n{h}  TailDesk-0.2.0-x86_64.AppImage\n{} *taildesk.zip\n",
            "00".repeat(32),
            "cd".repeat(32)
        );
        assert_eq!(
            hash_em_sums(&sums, "TailDesk-0.2.0-x86_64.AppImage"),
            Some([0xab; 32])
        );
        assert_eq!(hash_em_sums(&sums, "taildesk.zip"), Some([0xcd; 32]));
        assert_eq!(hash_em_sums(&sums, "outro"), None);
        assert_eq!(hash_em_sums("xyz  outro\n", "outro"), None);
    }

    #[test]
    fn baixa_confere_e_troca() {
        let pasta = std::env::temp_dir().join(format!("taildesk-update-{}", std::process::id()));
        fs::create_dir_all(&pasta).unwrap();
        let appimage = pasta.join("TailDesk.AppImage");
        fs::write(&appimage, b"velho").unwrap();
        let part = pasta.join("TailDesk.AppImage.part");
        let novo = b"conteudo novo do appimage".repeat(10_000);
        let esperado: [u8; 32] = Sha256::digest(&novo).into();

        // Hash errado: nada muda.
        let r = baixar(&mut &novo[..], &part, &[0; 32], 0, &mut |_, _| {});
        assert!(matches!(r, Err(Erro::HashDiferente)));
        assert_eq!(fs::read(&appimage).unwrap(), b"velho");

        let mut ultimo = 0;
        baixar(
            &mut &novo[..],
            &part,
            &esperado,
            novo.len() as u64,
            &mut |b, t| {
                assert_eq!(t, novo.len() as u64);
                ultimo = b;
            },
        )
        .unwrap();
        assert_eq!(ultimo, novo.len() as u64);
        trocar(&part, &appimage).unwrap();
        assert_eq!(fs::read(&appimage).unwrap(), novo);
        assert!(!part.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let modo = fs::metadata(&appimage).unwrap().permissions().mode();
            assert_eq!(modo & 0o777, 0o755);
        }
        fs::remove_dir_all(&pasta).unwrap();
    }
}
