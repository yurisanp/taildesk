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
}

impl PeerStatus {
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
    /// "Running" quando conectado.
    pub backend_state: String,
    #[serde(rename = "Self")]
    pub self_node: PeerStatus,
    /// Peers, indexados pela chave pública.
    pub peer: Option<std::collections::BTreeMap<String, PeerStatus>>,
}

impl Status {
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
            exchange(&mut stream, method, path).await
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LocalAPI não respondeu"))?
    }
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    method: &str,
    path: &str,
) -> io::Result<Vec<u8>> {
    // Host fixo exigido pela LocalAPI; sem Origin/Referer (proteção contra navegador).
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: local-tailscaled.sock\r\nUser-Agent: taildesk\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
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
      "Version": "1.104.1",
      "BackendState": "Running",
      "Self": {"ID": "nSelf", "HostName": "notebook-yuri", "DNSName": "notebook-yuri.tail1234.ts.net.",
               "OS": "linux", "TailscaleIPs": ["100.64.0.1", "fd7a:115c:a1e0::1"], "Online": true,
               "CurAddr": "", "Relay": "sao"},
      "Peer": {
        "nodekey:aa": {"ID": "nA", "HostName": "DESKTOP-CASA", "DNSName": "desktop-casa.tail1234.ts.net.",
                       "OS": "windows", "TailscaleIPs": ["100.64.0.2"], "Addrs": ["192.168.0.20:41641"],
                       "CurAddr": "192.168.0.20:41641", "Relay": "sao", "Online": true, "Tags": ["tag:remote-host"],
                       "RxBytes": 10, "Unknown": {"x": 1}},
        "nodekey:bb": {"ID": "nB", "HostName": "servidor", "DNSName": "servidor.tail1234.ts.net.",
                       "OS": "linux", "TailscaleIPs": ["100.64.0.3"], "Addrs": null,
                       "CurAddr": "", "Relay": "mia", "Online": false}
      }
    }"#;

    #[test]
    fn parses_status_subset() {
        let s: Status = serde_json::from_str(STATUS).unwrap();
        assert_eq!(s.backend_state, "Running");
        assert_eq!(s.self_node.short_name(), "notebook-yuri");
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
        let err = exchange(&mut client, "GET", "/localapi/v0/status")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("403"), "{err}");
        let req = srv.await.unwrap();
        assert!(req.starts_with("GET /localapi/v0/status HTTP/1.1\r\n"));
        assert!(req.contains("\r\nHost: local-tailscaled.sock\r\n"));
        assert!(!req.contains("Origin"));
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
