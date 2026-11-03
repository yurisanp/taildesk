//! `MobileVpnOverlay`: o app oficial do Tailscale no celular, sem LocalAPI
//! (`mobile/arquitetura.md` §3, ADR-M1).
//!
//! O app do Tailscale não expõe a LocalAPI a outros apps, então:
//! - o **estado** sai do que o Android mostra ([`SinaisRede`], lidos pela casca Kotlin com o
//!   `ConnectivityManager`) e de uma consulta ao DNS da tailnet (`100.100.100.100`), classificados
//!   por [`classificar`] (spike S4);
//! - a **lista de dispositivos** vem do desktop pareado (`PeerList`), mais favoritos e endereços
//!   digitados ([`MobileVpnOverlay::definir_dispositivos`]); é só sugestão;
//! - a **identidade** do host é o IP da tailnet mais o nome reverso no MagicDNS ([`ptr`],
//!   spike S5), ou o nome que a pessoa pareou. Serve para mostrar e fixar o certificado; quem
//!   autoriza a sessão é sempre o host, pelo `whois` do celular;
//! - o **caminho** (direto/relay) é o que o host informa no `PathInfo`;
//! - o **socket** é UDP do SO em `0.0.0.0`: o túnel do Tailscale roteia os 100.x.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::system::SystemSocket;
use crate::{Overlay, PathInfo, PeerIdentity, PeerInfo};

/// Resolvedor da tailnet (MagicDNS), sempre neste endereço.
pub const DNS_TAILNET: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(100, 100, 100, 100)), 53);
/// Versão mínima do Tailscale (a mesma do desktop, ADR-009). Abaixo disso só avisa.
pub const VERSAO_MINIMA: (u32, u32) = (1, 102);
/// Pacote do app oficial do Tailscale no Android.
pub const PACOTE_TAILSCALE: &str = "com.tailscale.ipn";

/// IP da tailnet: 100.64.0.0/10 (CGNAT) ou fd7a:115c:a1e0::/48.
pub fn ip_da_tailnet(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (o[1] & 0xC0) == 64
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Estado do Tailscale (S4)

/// O que o Android diz da rede, lido pela casca Kotlin a cada `NetworkCallback`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SinaisRede {
    /// `com.tailscale.ipn` instalado (`PackageManager`, com `<queries>` no manifesto).
    pub instalado: bool,
    /// `versionName` do app do Tailscale ("1.104.1").
    pub versao: Option<String>,
    /// A rede ativa deste app tem `TRANSPORT_VPN`.
    pub rede_vpn: bool,
    /// Endereços das interfaces de rede do aparelho (`NetworkInterface`), todos.
    pub enderecos: Vec<IpAddr>,
    /// `isActiveNetworkMetered`: 4G, hotspot ou Wi-Fi marcado como limitado.
    pub tarifada: bool,
    /// A consulta ao `100.100.100.100` feita deste processo respondeu. `None` = ainda não
    /// testou.
    pub dns_tailnet: Option<bool>,
}

impl SinaisRede {
    /// Primeiro IPv4 da tailnet entre as interfaces (senão o primeiro IPv6 da tailnet).
    pub fn ip_tailnet(&self) -> Option<IpAddr> {
        self.enderecos
            .iter()
            .copied()
            .filter(|&ip| ip_da_tailnet(ip))
            .min_by_key(|ip| ip.is_ipv6())
    }
}

/// Telas de estado do onboarding (mockups 01 e 18).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EstadoTailscale {
    /// Sem o app do Tailscale: botão "Instalar o Tailscale" (Play Store).
    NaoInstalado,
    /// Instalado, mas a VPN está desligada: "Abrir o Tailscale".
    Desligado,
    /// Outra VPN ocupa o lugar (o Android só deixa uma por vez).
    OutraVpn,
    /// O Tailscale está ligado, mas o TailDesk ficou de fora do túnel (split tunneling por app).
    ForaDoTunel,
    Conectado,
}

/// Resultado da classificação.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Classificacao {
    pub estado: EstadoTailscale,
    /// Tailscale abaixo de [`VERSAO_MINIMA`]: avisa, não bloqueia.
    pub versao_antiga: bool,
}

/// "1.104.1" → (1, 104).
pub fn versao_maior_menor(v: &str) -> Option<(u32, u32)> {
    let mut p = v.trim().split(['.', '-']);
    Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?))
}

/// Decide a tela a partir do que o Android mostra (`requisitos.md` §4.3).
///
/// - interface 100.x **e** rede ativa VPN **e** DNS da tailnet não falhou → conectado;
/// - interface 100.x sem a rede ativa ser VPN, ou com o DNS falhando → TailDesk fora do túnel
///   (o Android esconde a VPN dos apps excluídos, mas a interface continua visível);
/// - VPN ativa sem interface 100.x → outra VPN;
/// - o resto → desligado (ou não instalado).
pub fn classificar(s: &SinaisRede) -> Classificacao {
    let versao_antiga = s
        .versao
        .as_deref()
        .and_then(versao_maior_menor)
        .is_some_and(|v| v < VERSAO_MINIMA);
    let tailnet = s.ip_tailnet().is_some();
    let estado = if !s.instalado {
        EstadoTailscale::NaoInstalado
    } else if tailnet && s.rede_vpn && s.dns_tailnet != Some(false) {
        EstadoTailscale::Conectado
    } else if tailnet {
        EstadoTailscale::ForaDoTunel
    } else if s.rede_vpn {
        EstadoTailscale::OutraVpn
    } else {
        EstadoTailscale::Desligado
    };
    Classificacao {
        estado,
        versao_antiga,
    }
}

// ---------------------------------------------------------------------------------------------
// Nome reverso no MagicDNS (S5)

/// Consultas DNS mínimas: só PTR, só o que o MagicDNS responde.
pub mod ptr {
    use super::*;

    /// Nome reverso de um IP (`4.3.2.1.in-addr.arpa` ou os nibbles de `ip6.arpa`).
    pub fn nome_reverso(ip: IpAddr) -> String {
        match ip {
            IpAddr::V4(v4) => {
                let o = v4.octets();
                format!("{}.{}.{}.{}.in-addr.arpa", o[3], o[2], o[1], o[0])
            }
            IpAddr::V6(v6) => {
                let mut s = String::with_capacity(72);
                for b in v6.octets().iter().rev() {
                    s.push_str(&format!("{:x}.{:x}.", b & 0xf, b >> 4));
                }
                s.push_str("ip6.arpa");
                s
            }
        }
    }

    /// Pergunta PTR com o `id` dado (RD ligado).
    pub fn pergunta(id: u16, ip: IpAddr) -> Vec<u8> {
        let mut m = Vec::with_capacity(64);
        m.extend_from_slice(&id.to_be_bytes());
        m.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
        for rotulo in nome_reverso(ip).split('.') {
            m.push(rotulo.len() as u8);
            m.extend_from_slice(rotulo.as_bytes());
        }
        m.extend_from_slice(&[0, 0, 12, 0, 1]);
        m
    }

    /// Lê um nome (com compressão) a partir de `pos`. Devolve o nome e onde o campo acaba.
    fn ler_nome(m: &[u8], mut pos: usize) -> Option<(String, usize)> {
        let mut nome = String::new();
        let mut fim = None;
        // Limite de saltos contra laço de ponteiros.
        for _ in 0..64 {
            let len = *m.get(pos)? as usize;
            if len & 0xC0 == 0xC0 {
                let alvo = (len & 0x3F) << 8 | *m.get(pos + 1)? as usize;
                fim.get_or_insert(pos + 2);
                pos = alvo;
                continue;
            }
            if len == 0 {
                return Some((nome, fim.unwrap_or(pos + 1)));
            }
            let rotulo = m.get(pos + 1..pos + 1 + len)?;
            if !nome.is_empty() {
                nome.push('.');
            }
            nome.push_str(std::str::from_utf8(rotulo).ok()?);
            if nome.len() > 253 {
                return None;
            }
            pos += 1 + len;
        }
        None
    }

    /// O nome da primeira resposta PTR (sem o ponto final), se a resposta for para `id`.
    pub fn resposta(id: u16, m: &[u8]) -> Option<String> {
        if m.len() < 12 || u16::from_be_bytes([m[0], m[1]]) != id || m[2] & 0x80 == 0 {
            return None;
        }
        if m[3] & 0x0F != 0 {
            return None; // NXDOMAIN e outros erros
        }
        let perguntas = u16::from_be_bytes([m[4], m[5]]);
        let respostas = u16::from_be_bytes([m[6], m[7]]);
        let mut pos = 12;
        for _ in 0..perguntas {
            pos = ler_nome(m, pos)?.1 + 4;
        }
        for _ in 0..respostas {
            let (_, p) = ler_nome(m, pos)?;
            let tipo = u16::from_be_bytes([*m.get(p)?, *m.get(p + 1)?]);
            let tam = u16::from_be_bytes([*m.get(p + 8)?, *m.get(p + 9)?]) as usize;
            let dados = p + 10;
            if tipo == 12 {
                let (nome, _) = ler_nome(m, dados)?;
                return Some(nome.trim_end_matches('.').to_owned());
            }
            pos = dados + tam;
        }
        None
    }

    /// Pergunta ao `servidor` o nome de `ip`, esperando até `prazo`.
    pub async fn consultar(
        servidor: SocketAddr,
        ip: IpAddr,
        prazo: Duration,
    ) -> io::Result<Option<String>> {
        let local: SocketAddr = if servidor.is_ipv4() {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        };
        let sock = tokio::net::UdpSocket::bind(local).await?;
        // Id variado o bastante para não aceitar resposta velha de outra consulta.
        let id = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos())
            >> 8) as u16;
        sock.send_to(&pergunta(id, ip), servidor).await?;
        let mut buf = [0u8; 1500];
        let espera = async {
            loop {
                let (n, de) = sock.recv_from(&mut buf).await?;
                if de == servidor
                    && let Some(nome) = resposta(id, &buf[..n])
                {
                    return Ok::<_, io::Error>(Some(nome));
                }
                if de == servidor
                    && buf[..n].len() >= 2
                    && u16::from_be_bytes([buf[0], buf[1]]) == id
                {
                    return Ok(None);
                }
            }
        };
        match tokio::time::timeout(prazo, espera).await {
            Ok(r) => r,
            Err(_) => Err(io::ErrorKind::TimedOut.into()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Overlay

#[derive(Debug, Default)]
struct Estado {
    sinais: SinaisRede,
    dispositivos: Vec<PeerInfo>,
    /// Nome que a pessoa deu ou pareou para um IP (vale quando o PTR não responde).
    nomes: HashMap<IpAddr, String>,
    /// Último `PathInfo` de cada host.
    caminhos: HashMap<IpAddr, PathInfo>,
}

/// Overlay do celular. Clonar compartilha o estado (a casca Kotlin atualiza os sinais e a sessão
/// atualiza o caminho).
#[derive(Clone, Debug)]
pub struct MobileVpnOverlay {
    estado: Arc<Mutex<Estado>>,
    dns: SocketAddr,
    prazo_dns: Duration,
}

impl Default for MobileVpnOverlay {
    fn default() -> Self {
        MobileVpnOverlay::new(DNS_TAILNET)
    }
}

impl MobileVpnOverlay {
    /// `dns` é o MagicDNS ([`DNS_TAILNET`]); os testes usam um servidor local.
    pub fn new(dns: SocketAddr) -> Self {
        MobileVpnOverlay {
            estado: Arc::default(),
            dns,
            prazo_dns: Duration::from_millis(800),
        }
    }

    fn travar(&self) -> MutexGuard<'_, Estado> {
        self.estado.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Novos sinais do Android. Devolve a classificação para a interface.
    pub fn definir_sinais(&self, sinais: SinaisRede) -> Classificacao {
        let c = classificar(&sinais);
        self.travar().sinais = sinais;
        c
    }

    pub fn sinais(&self) -> SinaisRede {
        self.travar().sinais.clone()
    }

    pub fn classificacao(&self) -> Classificacao {
        classificar(&self.travar().sinais)
    }

    /// Testa o DNS da tailnet deste processo (parte do "conectado") e guarda o resultado.
    /// Pergunta o nome do próprio IP: qualquer resposta, mesmo vazia, prova que o túnel passa.
    pub async fn testar_dns(&self) -> bool {
        let Some(ip) = self.travar().sinais.ip_tailnet() else {
            self.travar().sinais.dns_tailnet = Some(false);
            return false;
        };
        let ok = ptr::consultar(self.dns, ip, self.prazo_dns).await.is_ok();
        self.travar().sinais.dns_tailnet = Some(ok);
        ok
    }

    /// Lista vinda do desktop pareado, mais favoritos e endereços digitados.
    pub fn definir_dispositivos(&self, lista: Vec<PeerInfo>) {
        self.travar().dispositivos = lista;
    }

    /// Nome escolhido ou pareado para um IP.
    pub fn nomear(&self, ip: IpAddr, nome: impl Into<String>) {
        self.travar().nomes.insert(ip, nome.into());
    }

    /// `PathInfo` que o host mandou na sessão.
    pub fn informar_caminho(&self, ip: IpAddr, caminho: PathInfo) {
        self.travar().caminhos.insert(ip, caminho);
    }

    /// Nome do host: PTR no MagicDNS, senão o pareado, senão o IP.
    pub async fn nome_de(&self, ip: IpAddr) -> String {
        if let Ok(Some(n)) = ptr::consultar(self.dns, ip, self.prazo_dns).await
            && !n.is_empty()
        {
            return n;
        }
        self.travar()
            .nomes
            .get(&ip)
            .cloned()
            .unwrap_or_else(|| ip.to_string())
    }
}

impl Overlay for MobileVpnOverlay {
    type Socket = SystemSocket;

    async fn local_addr(&self) -> io::Result<IpAddr> {
        self.travar().sinais.ip_tailnet().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "o Tailscale deste celular não está conectado",
            )
        })
    }

    async fn udp_bind(&self, port: u16) -> io::Result<SystemSocket> {
        // Em 0.0.0.0: o túnel do Tailscale é quem leva os 100.x.
        let s = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).await?;
        Ok(SystemSocket(s))
    }

    async fn peers(&self) -> io::Result<Vec<PeerInfo>> {
        Ok(self.travar().dispositivos.clone())
    }

    /// Só para mostrar e fixar o certificado: sem `whois`, o id é o IP da tailnet (estável por
    /// nó) e não há usuário.
    async fn identify(&self, remote: SocketAddr) -> io::Result<PeerIdentity> {
        let ip = remote.ip();
        if !ip_da_tailnet(ip) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{ip} não é um endereço da tailnet"),
            ));
        }
        let nome = self.nome_de(ip).await;
        Ok(PeerIdentity {
            node_stable_id: format!("ip:{ip}"),
            hostname: nome.split('.').next().unwrap_or_default().to_owned(),
            user_login: None,
            tags: Vec::new(),
            app_caps: Vec::new(),
        })
    }

    async fn path(&self, remote: IpAddr) -> PathInfo {
        self.travar()
            .caminhos
            .get(&remote)
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathKind;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn sinais(instalado: bool, vpn: bool, end: &[&str], dns: Option<bool>) -> SinaisRede {
        SinaisRede {
            instalado,
            versao: Some("1.104.1".into()),
            rede_vpn: vpn,
            enderecos: end.iter().map(|s| ip(s)).collect(),
            tarifada: false,
            dns_tailnet: dns,
        }
    }

    #[test]
    fn faixa_da_tailnet() {
        assert!(ip_da_tailnet(ip("100.64.0.1")));
        assert!(ip_da_tailnet(ip("100.127.255.254")));
        assert!(!ip_da_tailnet(ip("100.128.0.1")));
        assert!(!ip_da_tailnet(ip("100.63.0.1")));
        assert!(!ip_da_tailnet(ip("10.0.0.1")));
        assert!(ip_da_tailnet(ip("fd7a:115c:a1e0::1")));
        assert!(!ip_da_tailnet(ip("fd7a:115c:a1e1::1")));
    }

    #[test]
    fn cada_estado_do_tailscale() {
        use EstadoTailscale::*;
        let wifi = "192.168.0.10";
        let tun = "100.101.102.104";
        let casos = [
            (sinais(false, false, &[wifi], None), NaoInstalado),
            (sinais(true, false, &[wifi], None), Desligado),
            // WireGuard, Proton… com 10.x no tun.
            (sinais(true, true, &[wifi, "10.2.0.2"], None), OutraVpn),
            // Excluído do túnel: a interface aparece, mas a rede deste app não é VPN.
            (sinais(true, false, &[wifi, tun], None), ForaDoTunel),
            (sinais(true, true, &[wifi, tun], Some(false)), ForaDoTunel),
            (sinais(true, true, &[wifi, tun], None), Conectado),
            (
                sinais(true, true, &[wifi, "fd7a:115c:a1e0::9", tun], Some(true)),
                Conectado,
            ),
        ];
        for (s, esperado) in casos {
            assert_eq!(classificar(&s).estado, esperado, "{s:?}");
        }
    }

    #[test]
    fn versao_antiga_so_avisa() {
        let mut s = sinais(true, true, &["100.64.0.5"], Some(true));
        s.versao = Some("1.98.0".into());
        let c = classificar(&s);
        assert_eq!(c.estado, EstadoTailscale::Conectado);
        assert!(c.versao_antiga);
        s.versao = Some("1.102.0-t1234".into());
        assert!(!classificar(&s).versao_antiga);
        s.versao = Some("estranha".into());
        assert!(!classificar(&s).versao_antiga);
        assert_eq!(
            s.ip_tailnet(),
            Some(ip("100.64.0.5")),
            "IPv4 da tailnet tem preferência"
        );
    }

    #[test]
    fn nomes_reversos() {
        assert_eq!(
            ptr::nome_reverso(ip("100.101.102.103")),
            "103.102.101.100.in-addr.arpa"
        );
        let v6 = ptr::nome_reverso(ip("fd7a:115c:a1e0::1"));
        assert!(
            v6.starts_with("1.0.0.0.") && v6.ends_with("a.7.d.f.ip6.arpa"),
            "{v6}"
        );
    }

    /// Resposta PTR montada como o MagicDNS manda (com compressão do nome da pergunta).
    fn resposta_ptr(id: u16, q: &[u8], nome: &str) -> Vec<u8> {
        let mut r = q.to_vec();
        r[0..2].copy_from_slice(&id.to_be_bytes());
        r[2] = 0x81;
        r[3] = 0x80;
        r[6..8].copy_from_slice(&1u16.to_be_bytes());
        r.extend_from_slice(&[0xC0, 12, 0, 12, 0, 1, 0, 0, 0, 60]);
        let mut rdata = Vec::new();
        for rot in nome.split('.') {
            rdata.push(rot.len() as u8);
            rdata.extend_from_slice(rot.as_bytes());
        }
        rdata.push(0);
        r.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        r.extend_from_slice(&rdata);
        r
    }

    #[test]
    fn le_resposta_ptr_e_recusa_lixo() {
        let q = ptr::pergunta(0x1234, ip("100.64.0.2"));
        let r = resposta_ptr(0x1234, &q, "servidor-homelab.tail1234.ts.net.");
        assert_eq!(
            ptr::resposta(0x1234, &r).as_deref(),
            Some("servidor-homelab.tail1234.ts.net")
        );
        // Id errado, pergunta (QR=0), NXDOMAIN e truncada.
        assert_eq!(ptr::resposta(0x9999, &r), None);
        assert_eq!(ptr::resposta(0x1234, &q), None);
        let mut nx = r.clone();
        nx[3] = 0x83;
        assert_eq!(ptr::resposta(0x1234, &nx), None);
        for corte in 0..r.len() {
            let _ = ptr::resposta(0x1234, &r[..corte]);
        }
        // Ponteiro que aponta para ele mesmo não trava.
        let mut laco = r.clone();
        let n = laco.len();
        laco[n - 3] = 0xC0;
        laco[n - 2] = (n - 3) as u8;
        let _ = ptr::resposta(0x1234, &laco);
    }

    /// MagicDNS falso em localhost.
    async fn dns_falso(nome: &'static str) -> SocketAddr {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            while let Ok((n, de)) = sock.recv_from(&mut buf).await {
                let id = u16::from_be_bytes([buf[0], buf[1]]);
                let r = resposta_ptr(id, &buf[..n], nome);
                let _ = sock.send_to(&r, de).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn identifica_pelo_ptr_ou_pelo_nome_pareado() {
        let dns = dns_falso("pc-mae.tail1234.ts.net.").await;
        let o = MobileVpnOverlay::new(dns);
        let host: SocketAddr = "100.70.1.2:7420".parse().unwrap();
        let id = o.identify(host).await.unwrap();
        assert_eq!(id.hostname, "pc-mae");
        assert_eq!(id.node_stable_id, "ip:100.70.1.2");
        assert_eq!(id.user_login, None);
        // Fora da tailnet não é identificado.
        assert!(
            o.identify("192.168.0.5:7420".parse().unwrap())
                .await
                .is_err()
        );

        // Sem MagicDNS (porta fechada): vale o nome pareado.
        let mut sem = MobileVpnOverlay::new("127.0.0.1:9".parse().unwrap());
        sem.prazo_dns = Duration::from_millis(100);
        sem.nomear(host.ip(), "Notebook do Yuri");
        assert_eq!(sem.nome_de(host.ip()).await, "Notebook do Yuri");
    }

    #[tokio::test]
    async fn estado_caminho_e_dispositivos() {
        let dns = dns_falso("eu.tail1234.ts.net.").await;
        let o = MobileVpnOverlay::new(dns);
        assert!(o.local_addr().await.is_err());
        let c = o.definir_sinais(sinais(true, true, &["100.64.0.9"], None));
        assert_eq!(c.estado, EstadoTailscale::Conectado);
        assert!(o.testar_dns().await);
        assert_eq!(o.sinais().dns_tailnet, Some(true));
        assert_eq!(o.local_addr().await.unwrap(), ip("100.64.0.9"));

        let h = ip("100.64.0.2");
        assert_eq!(o.path(h).await.kind, PathKind::Unknown);
        o.informar_caminho(
            h,
            PathInfo {
                kind: PathKind::Relayed,
                derp_region: Some("sao".into()),
                ..PathInfo::default()
            },
        );
        assert_eq!(o.path(h).await.kind, PathKind::Relayed);

        o.definir_dispositivos(vec![PeerInfo {
            hostname: "servidor".into(),
            dns_name: String::new(),
            addr: h,
            os: None,
            online: true,
            tags: Vec::new(),
        }]);
        assert_eq!(o.peers().await.unwrap().len(), 1);
        let s = o.udp_bind(0).await.unwrap();
        assert!(
            crate::DatagramSocket::local_addr(&s)
                .unwrap()
                .ip()
                .is_unspecified()
        );
    }
}
