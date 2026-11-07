//! "Ligar" um computador desligado da lista (Wake on LAN, `17-wake-on-lan.md` §4.2–4.3).
//!
//! O MAC e a LAN do alvo vêm do último `PowerInfo` dele (guardado em [`crate::config`]). Quem
//! manda o pacote, em ordem:
//! 1. este computador, se ele estiver na mesma LAN;
//! 2. o TailDesk de outro dispositivo ligado (`WakeRequest`), primeiro os que anunciam um
//!    endereço na LAN do alvo; quem responde `NOT_SAME_LAN` passa a vez;
//! 3. o próprio `tailscaled` de um dispositivo na LAN do alvo, pela PeerAPI `/v0/wol`, que
//!    funciona mesmo sem o TailDesk lá (um roteador, um NAS).

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use taildesk_proto::control::{LanIface, WakeError, WakeRequest, WolState};
use taildesk_session::acordar::{self, Mac};
use taildesk_session::conexao::PORTA;
use taildesk_session::conexao::servico::pedir_acordar;
use taildesk_transport::{Pin, client_endpoint};
use taildesk_tsapi::{LocalApi, PeerStatus};

use crate::config;

/// Quanto esperar cada repetidor do TailDesk antes de passar ao próximo.
const ESPERA_REPETIDOR: Duration = Duration::from_secs(4);

/// Quem mandou o pacote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Por {
    EsteComputador,
    /// Nome do dispositivo que repetiu (TailDesk ou só Tailscale).
    Repetidor(String),
}

/// Um peer ligado que pode servir de repetidor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidato {
    pub id: String,
    pub nome: String,
    pub ip: IpAddr,
    /// Anuncia um endereço na LAN do alvo.
    pub na_lan: bool,
    /// Já rodou o TailDesk (está nos hosts conhecidos).
    pub taildesk: bool,
    pub peerapi: Option<SocketAddr>,
}

/// Peers ligados (menos o alvo), os da LAN do alvo primeiro, depois os com TailDesk.
pub fn candidatos(
    peers: &[&PeerStatus],
    alvo_id: &str,
    lan: &LanIface,
    conhecidos: &dyn Fn(&str) -> bool,
) -> Vec<Candidato> {
    let mut v: Vec<Candidato> = peers
        .iter()
        .filter(|p| p.online && p.id != alvo_id)
        .filter_map(|p| {
            let ip = *p.tailscale_ips.iter().find(|ip| ip.is_ipv4())?;
            Some(Candidato {
                id: p.id.clone(),
                nome: p.short_name().to_owned(),
                ip,
                na_lan: p
                    .underlay_ips()
                    .into_iter()
                    .any(|u| acordar::ip_na_lan(u, lan)),
                taildesk: conhecidos(&p.id),
                peerapi: p.peer_api_v4(),
            })
        })
        .collect();
    v.sort_by_key(|c| (!c.na_lan, !c.taildesk));
    v
}

/// Aviso para mostrar ao lado do botão "Ligar", pelas placas que o host informou.
pub fn aviso(placas: &[LanIface]) -> &'static str {
    match acordar::placa_para_acordar(placas) {
        None => "",
        Some((_, p)) if !p.wired => "A placa é Wi-Fi: a maioria dos computadores não acorda assim.",
        Some((_, p)) if p.wol() == WolState::Off => {
            "O Wake on LAN está desligado na placa de rede deste computador."
        }
        Some(_) => "",
    }
}

/// Acorda `alvo_id` (`node_stable_id`). `Err` traz o texto para a pessoa.
pub async fn ligar(alvo_id: &str, nome: &str) -> Result<Por, String> {
    let placas = config::ler().energia.remove(alvo_id).unwrap_or_default();
    let Some((mac, placa)) = acordar::placa_para_acordar(&placas) else {
        return Err(format!(
            "O TailDesk ainda não sabe a placa de rede de {nome}: conecte uma vez com ele ligado."
        ));
    };
    let lan = LanIface {
        mac: Vec::new(),
        ..placa.clone()
    };

    // 1. Este computador na mesma LAN.
    let locais = tokio::task::spawn_blocking(taildesk_platform::rede::placas_lan)
        .await
        .unwrap_or_default();
    let minhas: Vec<LanIface> = acordar::placas_para(&locais, Some(&lan))
        .into_iter()
        .cloned()
        .collect();
    if !minhas.is_empty() {
        let enviou = tokio::task::spawn_blocking(move || {
            let refs: Vec<&LanIface> = minhas.iter().collect();
            acordar::enviar(mac, &refs)
        })
        .await;
        if matches!(enviou, Ok(Ok(n)) if n > 0) {
            return Ok(Por::EsteComputador);
        }
    }

    // 2 e 3. Um repetidor na tailnet.
    let status = LocalApi::default()
        .status()
        .await
        .map_err(|e| format!("Não foi possível consultar o Tailscale: {e}"))?;
    let hosts = config::ler().hosts;
    let peers: Vec<&PeerStatus> = status.peers().collect();
    let lista = candidatos(&peers, alvo_id, &lan, &|id| hosts.contains_key(id));
    if let Some(por) = pelo_taildesk(&lista, mac, &lan, &hosts).await {
        return Ok(por);
    }
    if let Some(por) = pela_peerapi(&lista, mac).await {
        return Ok(por);
    }
    Err(format!(
        "Nenhum dispositivo ligado na mesma rede de {nome} conseguiu acordá-lo. Deixe outro \
         computador ou um roteador com Tailscale ligado lá."
    ))
}

async fn pelo_taildesk(
    lista: &[Candidato],
    mac: Mac,
    lan: &LanIface,
    hosts: &std::collections::BTreeMap<String, taildesk_transport::tls::Fingerprint>,
) -> Option<Por> {
    for c in lista.iter().filter(|c| c.na_lan || c.taildesk) {
        let remoto = SocketAddr::new(c.ip, PORTA);
        let Ok(endpoint) = client_endpoint(remoto) else {
            continue;
        };
        let pin = hosts
            .get(&c.id)
            .copied()
            .map_or(Pin::FirstContact, Pin::Sha256);
        let pedido = WakeRequest {
            mac: mac.to_vec(),
            lan: Some(lan.clone()),
        };
        let r = tokio::time::timeout(
            ESPERA_REPETIDOR,
            pedir_acordar(&endpoint, remoto, pin, pedido),
        )
        .await;
        endpoint.close(0u32.into(), b"fim");
        match r {
            Ok(Ok(r)) if r.ok => return Some(Por::Repetidor(c.nome.clone())),
            // Pediu agora há pouco por outro caminho: o pacote já saiu.
            Ok(Ok(r)) if r.error() == WakeError::RateLimited => {
                return Some(Por::Repetidor(c.nome.clone()));
            }
            // `NOT_SAME_LAN`, recusa, sem TailDesk ou sem resposta: o próximo.
            _ => {}
        }
    }
    None
}

async fn pela_peerapi(lista: &[Candidato], mac: Mac) -> Option<Por> {
    let texto = acordar::mac_texto(mac);
    for c in lista.iter().filter(|c| c.na_lan) {
        let Some(api) = c.peerapi else { continue };
        match taildesk_tsapi::peerapi_wol(api, &texto).await {
            Ok(r) if r.sent_to.as_ref().is_some_and(|s| !s.is_empty()) => {
                return Some(Por::Repetidor(c.nome.clone()));
            }
            Ok(_) => {}
            Err(e) => eprintln!("taildesk: wake on lan pela PeerAPI de {}: {e}", c.nome),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, ip: &str, online: bool, addrs: &[&str]) -> PeerStatus {
        PeerStatus {
            id: id.into(),
            host_name: id.into(),
            tailscale_ips: vec![ip.parse().unwrap()],
            online,
            addrs: Some(addrs.iter().map(|a| (*a).to_owned()).collect()),
            peer_api_url: Some(vec![format!("http://{ip}:41112")]),
            ..PeerStatus::default()
        }
    }

    #[test]
    fn ordem_dos_repetidores() {
        let lan = LanIface {
            ipv4: u32::from(std::net::Ipv4Addr::new(192, 168, 0, 20)),
            prefix_len: 24,
            ..LanIface::default()
        };
        let peers = [
            peer("nLonge", "100.64.0.5", true, &["200.1.2.3:41641"]),
            peer("nAlvo", "100.64.0.2", false, &["192.168.0.20:41641"]),
            peer("nNas", "100.64.0.3", true, &["192.168.0.40:41641"]),
            peer("nDesligado", "100.64.0.4", false, &["192.168.0.41:41641"]),
            peer("nTaildesk", "100.64.0.6", true, &[]),
        ];
        let refs: Vec<&PeerStatus> = peers.iter().collect();
        let v = candidatos(&refs, "nAlvo", &lan, &|id| id == "nTaildesk");
        let ids: Vec<&str> = v.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["nNas", "nTaildesk", "nLonge"]);
        assert!(v[0].na_lan && !v[0].taildesk);
        assert_eq!(v[0].peerapi, Some("100.64.0.3:41112".parse().unwrap()));
        assert!(!v[1].na_lan && v[1].taildesk);
    }

    #[test]
    fn avisos() {
        let placa = |wired, wol: WolState| LanIface {
            mac: vec![2, 0, 0, 0, 0, 1],
            wired,
            wol: wol.into(),
            ..LanIface::default()
        };
        assert_eq!(aviso(&[]), "");
        assert_eq!(aviso(&[placa(true, WolState::Armed)]), "");
        assert_eq!(aviso(&[placa(true, WolState::Unknown)]), "");
        assert!(aviso(&[placa(false, WolState::Armed)]).contains("Wi-Fi"));
        assert!(aviso(&[placa(true, WolState::Off)]).contains("desligado"));
    }
}
