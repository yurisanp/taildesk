//! Cliente fino da LocalAPI do `tailscaled` instalado na máquina: `status`, `ping` e `whois`.
//!
//! O TailDesk usa só o Tailscale do sistema (decisão do Yuri em 15/10/2026). A LocalAPI é
//! HTTP/1.1 sobre o socket local do `tailscaled`: unix socket no Linux, named pipe no Windows.
//! Este cliente faz uma requisição por conexão (`Connection: close`), com teto de tamanho da
//! resposta, e só lê os campos de que o TailDesk precisa.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

mod http;

pub use http::parse_response;

/// Maior resposta aceita (o `status` de uma tailnet grande tem centenas de KB).
pub const MAX_RESPONSE: usize = 16 * 1024 * 1024;

/// Tempo máximo de uma requisição (o `ping` espera a resposta do peer).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Caminho padrão do socket do `tailscaled`.
pub fn default_socket() -> &'static str {
    if cfg!(windows) {
        r"\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled"
    } else {
        "/var/run/tailscale/tailscaled.sock"
    }
}

/// Cliente da LocalAPI.
#[derive(Clone, Debug)]
pub struct LocalApi {
    socket: String,
}

impl Default for LocalApi {
    fn default() -> Self {
        LocalApi::new(default_socket())
    }
}

/// Um nó da tailnet como o `status` descreve (subconjunto de `ipnstate.PeerStatus`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct PeerStatus {
    #[serde(rename = "ID")]
    pub id: String,
    pub host_name: String,
    #[serde(rename = "DNSName")]
    pub dns_name: String,
    #[serde(rename = "OS")]
    pub os: String,
    #[serde(rename = "TailscaleIPs")]
    pub tailscale_ips: Vec<IpAddr>,
    /// Endereços de underlay conhecidos ("ip:porta").
    pub addrs: Option<Vec<String>>,
    /// Endereço direto em uso agora; vazio = sem caminho direto (relay ou ocioso).
    pub cur_addr: String,
    /// Região DERP de referência do nó (não quer dizer que o tráfego passa por ela).
    pub relay: String,
    pub online: bool,
    pub tags: Option<Vec<String>>,
    /// Última vez visto pelo servidor de controle (RFC 3339; ano 1 quando nunca).
    pub last_seen: String,
    /// Dono do nó (chave de [`Status::user`]).
    #[serde(rename = "UserID")]
    pub user_id: u64,
    /// PeerAPI do `tailscaled` do nó (`http://100.x.y.z:porta`), uma por IP da tailnet.
    #[serde(rename = "PeerAPIURL")]
    pub peer_api_url: Option<Vec<String>>,
}

impl PeerStatus {
    /// IPs de underlay que o nó anuncia (`Addrs` e `CurAddr`), sem repetir: os da LAN dele
    /// aparecem aqui, o que serve para achar um repetidor do Wake on LAN.
    pub fn underlay_ips(&self) -> Vec<IpAddr> {
        let mut ips: Vec<IpAddr> = self
            .addrs
            .iter()
            .flatten()
            .chain(std::iter::once(&self.cur_addr))
            .filter_map(|a| a.parse::<SocketAddr>().ok())
            .map(|a| a.ip())
            .collect();
        ips.sort();
        ips.dedup();
        ips
    }

    /// A PeerAPI em IPv4 (a do `/v0/wol`), se o nó anunciar.
    pub fn peer_api_v4(&self) -> Option<SocketAddr> {
        self.peer_api_url
            .iter()
            .flatten()
            .filter_map(|u| peerapi_addr(u))
            .find(SocketAddr::is_ipv4)
    }

    /// Nome curto: o primeiro rótulo do nome MagicDNS, ou o hostname.
    pub fn short_name(&self) -> &str {
        self.dns_name
            .split('.')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.host_name)
    }
}

/// Resposta do `status` (subconjunto de `ipnstate.Status`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct Status {
    /// Versão do `tailscaled` (ex.: "1.104.1-t…").
    pub version: String,
    /// "Running" quando conectado; "NeedsLogin", "Stopped", "Starting" etc. nos outros casos.
    pub backend_state: String,
    /// Tailnet atual; ausente quando não há login.
    pub current_tailnet: Option<TailnetStatus>,
    #[serde(rename = "Self")]
    pub self_node: PeerStatus,
    /// Peers, indexados pela chave pública.
    pub peer: Option<std::collections::BTreeMap<String, PeerStatus>>,
    /// Perfis dos usuários, pelo `UserID` em texto.
    pub user: Option<std::collections::BTreeMap<String, UserProfile>>,
}

/// Perfil de um usuário da tailnet (subconjunto de `tailcfg.UserProfile`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct UserProfile {
    pub login_name: String,
    pub display_name: String,
}

/// Tailnet atual (subconjunto de `ipnstate.TailnetStatus`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct TailnetStatus {
    pub name: String,
    #[serde(rename = "MagicDNSSuffix")]
    pub magic_dns_suffix: String,
}

impl Status {
    /// Versão do `tailscaled` como (maior, menor), quando dá para ler.
    pub fn version_major_minor(&self) -> Option<(u32, u32)> {
        let mut partes = self.version.split(['.', '-']);
        let maior = partes.next()?.parse().ok()?;
        let menor = partes.next()?.parse().ok()?;
        Some((maior, menor))
    }

    /// Login do dono deste nó (`None` em nó com tag ou sem login).
    pub fn self_login(&self) -> Option<&str> {
        if self.self_node.tags.as_ref().is_some_and(|t| !t.is_empty()) {
            return None;
        }
        self.user
            .as_ref()?
            .get(&self.self_node.user_id.to_string())
            .map(|u| u.login_name.as_str())
            .filter(|l| !l.is_empty())
    }

    pub fn peers(&self) -> impl Iterator<Item = &PeerStatus> {
        self.peer.iter().flat_map(|m| m.values())
    }

    /// Peer dono deste IP da tailnet.
    pub fn peer_by_ip(&self, ip: IpAddr) -> Option<&PeerStatus> {
        self.peers().find(|p| p.tailscale_ips.contains(&ip))
    }

    /// Peer pelo nome curto, nome MagicDNS completo ou hostname (sem diferenciar maiúsculas).
    pub fn peer_by_name(&self, name: &str) -> Option<&PeerStatus> {
        let name = name.trim_end_matches('.');
        self.peers().find(|p| {
            p.short_name().eq_ignore_ascii_case(name)
                || p.dns_name.trim_end_matches('.').eq_ignore_ascii_case(name)
                || p.host_name.eq_ignore_ascii_case(name)
        })
    }
}

/// Resultado do `ping` do tipo disco (subconjunto de `ipnstate.PingResult`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct PingResult {
    #[serde(rename = "IP")]
    pub ip: String,
    pub node_name: String,
    pub err: String,
    pub latency_seconds: f64,
    /// "ip:porta" quando o ping foi direto.
    pub endpoint: String,
    /// Preenchido quando passou por um peer relay.
    pub peer_relay: String,
    /// Região DERP quando passou pelo relay.
    #[serde(rename = "DERPRegionCode")]
    pub derp_region_code: String,
}

impl PingResult {
    pub fn latency(&self) -> Option<Duration> {
        (self.err.is_empty() && self.latency_seconds > 0.0)
            .then(|| Duration::from_secs_f64(self.latency_seconds))
    }
}

/// Resposta do `whois` (subconjunto de `apitype.WhoIsResponse`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct WhoIs {
    pub node: WhoIsNode,
    pub user_profile: WhoIsUser,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct WhoIsNode {
    #[serde(rename = "StableID")]
    pub stable_id: String,
    /// Nome MagicDNS completo.
    pub name: String,
    pub tags: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct WhoIsUser {
    pub login_name: String,
    pub display_name: String,
}

/// Resposta do `POST /v0/wol` da PeerAPI (`feature/wakeonlan`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct WolResponse {
    /// Interfaces por onde o pacote saiu.
    pub sent_to: Option<Vec<String>>,
    pub errors: Option<Vec<String>>,
}

/// `http://100.64.0.2:41112` → endereço. Só `http` e IP literal (é o que o `tailscaled` publica).
fn peerapi_addr(url: &str) -> Option<SocketAddr> {
    let resto = url.strip_prefix("http://")?;
    resto.split('/').next()?.parse().ok()
}

/// Pede ao `tailscaled` do nó `peerapi` (da [`PeerStatus::peer_api_v4`]) que mande o pacote
/// mágico para `mac` (`aa:bb:cc:dd:ee:ff`) em todas as placas IPv4 dele. O próprio `tailscaled`
/// autoriza: mesmo usuário sem tags, ou a capability `tailscale.com/cap/wake-on-lan`; senão
/// volta [`io::ErrorKind::PermissionDenied`].
pub async fn peerapi_wol(peerapi: SocketAddr, mac: &str) -> io::Result<WolResponse> {
    let valido = mac.len() == 17
        && mac
            .split(':')
            .all(|b| b.len() == 2 && b.bytes().all(|c| c.is_ascii_hexdigit()));
    if !valido {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "MAC inválido"));
    }
    let body = tokio::time::timeout(REQUEST_TIMEOUT, async {
        let mut stream = tokio::net::TcpStream::connect(peerapi).await?;
        exchange(
            &mut stream,
            &peerapi.to_string(),
            "POST",
            &format!("/v0/wol?mac={mac}"),
        )
        .await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "PeerAPI não respondeu"))??;
    serde_json::from_slice(&body).map_err(invalid)
}

fn invalid(e: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

impl LocalApi {
    pub fn new(socket: impl Into<String>) -> LocalApi {
        LocalApi {
            socket: socket.into(),
        }
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }

    pub async fn status(&self) -> io::Result<Status> {
        let body = self.request("GET", "/localapi/v0/status").await?;
        serde_json::from_slice(&body).map_err(invalid)
    }

    /// Ping disco até `ip` (IP da tailnet): diz se o caminho é direto ou relay, e a latência.
    pub async fn ping(&self, ip: IpAddr) -> io::Result<PingResult> {
        let body = self
            .request("POST", &format!("/localapi/v0/ping?ip={ip}&type=disco"))
            .await?;
        serde_json::from_slice(&body).map_err(invalid)
    }

    /// Quem está do outro lado de `addr` (IP:porta da tailnet).
    pub async fn whois(&self, addr: SocketAddr) -> io::Result<WhoIs> {
        let body = self
            .request("GET", &format!("/localapi/v0/whois?addr={addr}"))
            .await?;
        serde_json::from_slice(&body).map_err(invalid)
    }

    async fn request(&self, method: &str, path: &str) -> io::Result<Vec<u8>> {
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            let mut stream = connect(&self.socket).await.map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!(
                        "Tailscale do sistema não encontrado em {} ({e}); instale e conecte o Tailscale",
                        self.socket
                    ),
                )
            })?;
            exchange(&mut stream, LOCALAPI_HOST, method, path).await
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LocalAPI não respondeu"))?
    }
}

/// Host fixo exigido pela LocalAPI.
const LOCALAPI_HOST: &str = "local-tailscaled.sock";

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    host: &str,
    method: &str,
    path: &str,
) -> io::Result<Vec<u8>> {
    // Sem Origin/Referer (a LocalAPI recusa pedidos com cara de navegador).
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: taildesk\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await?;
    stream.flush().await?;
    let mut raw = Vec::new();
    let mut limited = stream.take(MAX_RESPONSE as u64 + 1);
    limited.read_to_end(&mut raw).await?;
    if raw.len() > MAX_RESPONSE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "resposta da LocalAPI grande demais",
        ));
    }
    let (status, body) = parse_response(&raw)?;
    if status == 403 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("recusado (403): {}", String::from_utf8_lossy(&body).trim()),
        ));
    }
    if status != 200 {
        let msg = String::from_utf8_lossy(&body);
        return Err(io::Error::other(format!(
            "LocalAPI respondeu {status}: {}",
            msg.trim()
        )));
    }
    Ok(body)
}

#[cfg(unix)]
async fn connect(socket: &str) -> io::Result<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(socket).await
}

#[cfg(windows)]
async fn connect(socket: &str) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    tokio::net::windows::named_pipe::ClientOptions::new().open(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = r#"{
      "Version": "1.104.1-t2f6a1b0c3-g4d5e6f7a8",
      "BackendState": "Running",
      "CurrentTailnet": {"Name": "yuri@github", "MagicDNSSuffix": "tail1234.ts.net", "MagicDNSEnabled": true},
      "Self": {"ID": "nSelf", "HostName": "notebook-yuri", "DNSName": "notebook-yuri.tail1234.ts.net.",
               "OS": "linux", "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"], "Online": true,
               "CurAddr": "", "Relay": "sao", "UserID": 42},
      "User": {"42": {"ID": 42, "LoginName": "yuri@example.com", "DisplayName": "Yuri"}},
      "Peer": {
        "nodekey:aa": {"ID": "nA", "HostName": "DESKTOP-CASA", "DNSName": "desktop-casa.tail1234.ts.net.",
                       "OS": "windows", "TailscaleIPs": ["100.64.0.2"], "Addrs": ["192.168.0.20:41641"],
                       "CurAddr": "192.168.0.20:41641", "Relay": "sao", "Online": true, "Tags": ["tag:remote-host"],
                       "RxBytes": 10, "Unknown": {"x": 1}},
        "nodekey:bb": {"ID": "nB", "HostName": "servidor", "DNSName": "servidor.tail1234.ts.net.",
                       "OS": "linux", "TailscaleIPs": ["100.64.0.3"], "Addrs": null,
                       "CurAddr": "", "Relay": "mia", "Online": false, "LastSeen": "2026-10-14T21:03:00Z"}
      }
    }"#;

    #[test]
    fn parses_status_subset() {
        let s: Status = serde_json::from_str(STATUS).unwrap();
        assert_eq!(s.backend_state, "Running");
        assert_eq!(s.version_major_minor(), Some((1, 104)));
        assert_eq!(s.current_tailnet.as_ref().unwrap().name, "yuri@github");
        assert_eq!(
            s.peer_by_name("servidor").unwrap().last_seen,
            "2026-10-14T21:03:00Z"
        );
        assert_eq!(s.self_node.short_name(), "notebook-yuri");
        assert_eq!(s.self_login(), Some("yuri@example.com"));
        assert_eq!(s.peers().count(), 2);
        let casa = s.peer_by_name("desktop-casa").unwrap();
        assert_eq!(casa.cur_addr, "192.168.0.20:41641");
        assert_eq!(
            casa.tags.as_deref(),
            Some(&["tag:remote-host".to_owned()][..])
        );
        assert_eq!(s.peer_by_name("DESKTOP-CASA.tail1234.ts.net."), Some(casa));
        assert_eq!(
            s.peer_by_ip("100.64.0.3".parse().unwrap())
                .unwrap()
                .host_name,
            "servidor"
        );
        assert!(s.peer_by_name("outro").is_none());
    }

    #[test]
    fn parses_ping_and_whois() {
        let direct: PingResult = serde_json::from_str(
            r#"{"IP":"100.64.0.2","NodeIP":"100.64.0.2","NodeName":"desktop-casa","Err":"",
                "LatencySeconds":0.0021,"Endpoint":"192.168.0.20:41641","PeerRelay":"",
                "DERPRegionID":0,"DERPRegionCode":""}"#,
        )
        .unwrap();
        assert_eq!(direct.latency(), Some(Duration::from_secs_f64(0.0021)));
        let failed: PingResult =
            serde_json::from_str(r#"{"IP":"100.64.0.9","Err":"timeout","LatencySeconds":0}"#)
                .unwrap();
        assert_eq!(failed.latency(), None);

        let w: WhoIs = serde_json::from_str(
            r#"{"Node":{"ID":5,"StableID":"nA","Name":"desktop-casa.tail1234.ts.net.","Tags":null},
                "UserProfile":{"ID":1,"LoginName":"yuri@example.com","DisplayName":"Yuri"},
                "CapMap":{}}"#,
        )
        .unwrap();
        assert_eq!(w.node.stable_id, "nA");
        assert_eq!(w.user_profile.login_name, "yuri@example.com");
    }

    #[tokio::test]
    async fn exchange_speaks_http_and_reports_errors() {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        let srv = tokio::spawn(async move {
            let mut req = vec![0u8; 512];
            let n = server.read(&mut req).await.unwrap();
            let req = String::from_utf8_lossy(&req[..n]).to_string();
            server
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 11\r\n\r\naccess deny")
                .await
                .unwrap();
            req
        });
        let err = exchange(&mut client, LOCALAPI_HOST, "GET", "/localapi/v0/status")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("403"), "{err}");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        let req = srv.await.unwrap();
        assert!(req.starts_with("GET /localapi/v0/status HTTP/1.1\r\n"));
        assert!(req.contains("\r\nHost: local-tailscaled.sock\r\n"));
        assert!(!req.contains("Origin"));
    }

    #[test]
    fn underlay_and_peerapi() {
        let p: PeerStatus = serde_json::from_str(
            r#"{"ID":"nA","Addrs":["192.168.0.20:41641","200.1.2.3:41641","192.168.0.20:41641"],
                "CurAddr":"192.168.0.20:41641",
                "PeerAPIURL":["http://100.64.0.2:41112","http://[fd7a:115c:a1e0::2]:41112"]}"#,
        )
        .unwrap();
        assert_eq!(
            p.underlay_ips(),
            vec![
                "192.168.0.20".parse::<IpAddr>().unwrap(),
                "200.1.2.3".parse().unwrap()
            ]
        );
        assert_eq!(p.peer_api_v4(), Some("100.64.0.2:41112".parse().unwrap()));
        assert_eq!(PeerStatus::default().peer_api_v4(), None);
        assert!(PeerStatus::default().underlay_ips().is_empty());
        assert_eq!(peerapi_addr("https://100.64.0.2:1"), None);
        assert_eq!(peerapi_addr("http://nome:1"), None);
    }

    #[tokio::test]
    async fn peerapi_wol_pede_o_pacote() {
        let ouvinte = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = ouvinte.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            let (mut s, _) = ouvinte.accept().await.unwrap();
            let mut req = vec![0u8; 512];
            let n = s.read(&mut req).await.unwrap();
            let corpo = br#"{"SentTo":["enp3s0"],"Errors":null}"#;
            s.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", corpo.len()).as_bytes(),
            )
            .await
            .unwrap();
            s.write_all(corpo).await.unwrap();
            String::from_utf8_lossy(&req[..n]).to_string()
        });
        let r = peerapi_wol(addr, "3c:7c:3f:01:ab:03").await.unwrap();
        assert_eq!(r.sent_to, Some(vec!["enp3s0".into()]));
        let req = srv.await.unwrap();
        assert!(req.starts_with("POST /v0/wol?mac=3c:7c:3f:01:ab:03 HTTP/1.1\r\n"));
        assert!(req.contains(&format!("\r\nHost: {addr}\r\n")));
        // MAC estranho nem sai da máquina.
        for mac in [
            "3c:7c:3f:01:ab",
            "3c:7c:3f:01:ab:0g",
            "3c:7c:3f:01:ab:03&x=1",
        ] {
            assert_eq!(
                peerapi_wol(addr, mac).await.unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[tokio::test]
    async fn missing_daemon_is_a_clear_error() {
        let api = LocalApi::new(if cfg!(windows) {
            r"\\.\pipe\taildesk-nao-existe"
        } else {
            "/nao/existe/tailscaled.sock"
        });
        let err = api.status().await.unwrap_err();
        assert!(err.to_string().contains("Tailscale do sistema"), "{err}");
    }
}
