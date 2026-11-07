//! Wake on LAN pela tailnet (`17-wake-on-lan.md` §4).
//!
//! O pacote mágico é broadcast de camada 2 e não atravessa a tailnet: quem manda é um nó
//! acordado na mesma LAN do alvo (o repetidor). Este módulo tem a parte pura (montar o pacote,
//! decidir se duas placas estão na mesma LAN, limitar a taxa) e o envio por UDP; quem escolhe o
//! repetidor é o controlador, e quem atende o pedido é o host ([`crate::conexao::host`]).

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use taildesk_proto::control::{LanIface, WolState};

/// Porta do pacote mágico ("discard"); a 7 também é comum, mas a 9 é a que todos escutam.
pub const PORTA: u16 = 9;
/// Quantas vezes o pacote sai por placa (UDP pode se perder; o alvo ignora repetidos).
pub const REPETICOES: u32 = 3;
/// Intervalo entre as repetições.
pub const INTERVALO: Duration = Duration::from_millis(100);
/// Um pedido por MAC a cada tanto (`17-wake-on-lan.md` §4.4).
pub const INTERVALO_POR_MAC: Duration = Duration::from_secs(10);

/// Um MAC Ethernet.
pub type Mac = [u8; 6];

/// `FF` × 6 e o MAC 16 vezes: 102 bytes.
pub fn pacote_magico(mac: Mac) -> [u8; 102] {
    let mut p = [0xFF; 102];
    for bloco in p[6..].chunks_exact_mut(6) {
        bloco.copy_from_slice(&mac);
    }
    p
}

/// MAC de um campo `bytes` do protocolo (6 bytes, nem todo zero, nem broadcast).
pub fn mac_de(bytes: &[u8]) -> Option<Mac> {
    let mac: Mac = bytes.try_into().ok()?;
    (mac != [0; 6] && mac != [0xFF; 6]).then_some(mac)
}

/// `aa:bb:cc:dd:ee:ff`, como o Tailscale e o `ip link` escrevem.
pub fn mac_texto(mac: Mac) -> String {
    let [a, b, c, d, e, f] = mac;
    format!("{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}")
}

/// Máscara de um prefixo IPv4 (0–32).
fn mascara(prefixo: u32) -> u32 {
    match prefixo {
        0 => 0,
        p => u32::MAX << (32 - p.min(32)),
    }
}

/// Broadcast dirigido da sub-rede da placa (`192.168.0.255` para `192.168.0.20/24`). `None`
/// sem endereço ou com prefixo /31 e /32, que não têm broadcast.
pub fn broadcast_dirigido(iface: &LanIface) -> Option<Ipv4Addr> {
    if iface.ipv4 == 0 || iface.prefix_len == 0 || iface.prefix_len > 30 {
        return None;
    }
    Some(Ipv4Addr::from(iface.ipv4 | !mascara(iface.prefix_len)))
}

/// `local` está na LAN de `alvo`? Mesma sub-rede (pelo prefixo do alvo) e, quando os dois sabem,
/// o mesmo MAC de gateway: duas casas com `192.168.0.0/24` não se confundem.
pub fn mesma_lan(local: &LanIface, alvo: &LanIface) -> bool {
    if local.ipv4 == 0 || alvo.ipv4 == 0 || alvo.prefix_len == 0 {
        return false;
    }
    let m = mascara(alvo.prefix_len);
    if local.ipv4 & m != alvo.ipv4 & m {
        return false;
    }
    match (mac_de(&local.gateway_mac), mac_de(&alvo.gateway_mac)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// `ip` (um endereço de underlay que um peer anuncia) cai na sub-rede de `alvo`? Pista de que
/// o peer está na mesma LAN e serve de repetidor.
pub fn ip_na_lan(ip: IpAddr, alvo: &LanIface) -> bool {
    let IpAddr::V4(ip) = ip else {
        return false;
    };
    if alvo.ipv4 == 0 || alvo.prefix_len == 0 || ip.is_loopback() || ip.is_unspecified() {
        return false;
    }
    let m = mascara(alvo.prefix_len);
    u32::from(ip) & m == alvo.ipv4 & m
}

/// A placa a acordar dentre as que o host informou: cabo antes de Wi-Fi e, entre iguais, a que
/// não está desligada. `None` sem nenhuma com MAC válido.
pub fn placa_para_acordar(placas: &[LanIface]) -> Option<(Mac, &LanIface)> {
    placas
        .iter()
        .filter_map(|p| mac_de(&p.mac).map(|m| (m, p)))
        .min_by_key(|(_, p)| (!p.wired, p.wol() == WolState::Off))
}

/// Por onde mandar: as placas com IPv4 na LAN do alvo (ou todas com IPv4, sem `alvo`).
pub fn placas_para<'a>(locais: &'a [LanIface], alvo: Option<&LanIface>) -> Vec<&'a LanIface> {
    locais
        .iter()
        .filter(|l| l.ipv4 != 0)
        .filter(|l| alvo.is_none_or(|a| mesma_lan(l, a)))
        .collect()
}

/// Manda o pacote mágico de cada placa para `255.255.255.255` e para o broadcast dirigido dela,
/// [`REPETICOES`] vezes. Bloqueia por uns 200 ms: chame fora do executor
/// (`tokio::task::spawn_blocking`). Devolve por quantas placas saiu ao menos uma vez.
pub fn enviar(mac: Mac, placas: &[&LanIface]) -> io::Result<u32> {
    let pacote = pacote_magico(mac);
    let mut socks = Vec::new();
    let mut ultimo_erro = None;
    for p in placas {
        // Presa ao IP da placa, a saída vai por ela (e não pela rota padrão, nem pela tailnet).
        let aberto = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::from(p.ipv4), 0))
            .and_then(|s| s.set_broadcast(true).map(|()| s));
        match aberto {
            Ok(s) => socks.push((s, broadcast_dirigido(p))),
            Err(e) => ultimo_erro = Some(e),
        }
    }
    let mut saiu = vec![false; socks.len()];
    for vez in 0..REPETICOES {
        if vez > 0 {
            std::thread::sleep(INTERVALO);
        }
        for ((s, dirigido), ok) in socks.iter().zip(saiu.iter_mut()) {
            for destino in std::iter::once(Ipv4Addr::BROADCAST).chain(*dirigido) {
                match s.send_to(&pacote, SocketAddrV4::new(destino, PORTA)) {
                    Ok(_) => *ok = true,
                    Err(e) => ultimo_erro = Some(e),
                }
            }
        }
    }
    let n = saiu.iter().filter(|&&ok| ok).count();
    match (n, ultimo_erro) {
        (0, Some(e)) => Err(e),
        (n, _) => Ok(u32::try_from(n).unwrap_or(u32::MAX)),
    }
}

/// Limite de pedidos por MAC (o repetidor não vira canhão de broadcast).
#[derive(Debug, Default)]
pub struct Limite {
    ultimo: HashMap<Mac, Instant>,
}

impl Limite {
    /// `true` = pode mandar agora (e conta este pedido).
    pub fn liberar(&mut self, mac: Mac, agora: Instant) -> bool {
        self.ultimo
            .retain(|_, t| agora.saturating_duration_since(*t) < INTERVALO_POR_MAC);
        if self.ultimo.contains_key(&mac) {
            return false;
        }
        self.ultimo.insert(mac, agora);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(ip: [u8; 4], prefixo: u32, gw: &[u8]) -> LanIface {
        LanIface {
            mac: vec![2, 0, 0, 0, 0, 1],
            ipv4: u32::from(Ipv4Addr::from(ip)),
            prefix_len: prefixo,
            gateway_mac: gw.to_vec(),
            wired: true,
            wol: 0,
        }
    }

    #[test]
    fn pacote_tem_102_bytes_e_o_mac_16_vezes() {
        let mac = [0x3c, 0x7c, 0x3f, 1, 2, 3];
        let p = pacote_magico(mac);
        assert_eq!(p.len(), 102);
        assert_eq!(&p[..6], &[0xFF; 6]);
        for i in 0..16 {
            assert_eq!(&p[6 + i * 6..12 + i * 6], &mac);
        }
    }

    #[test]
    fn mac_valido() {
        assert_eq!(mac_de(&[1, 2, 3, 4, 5, 6]), Some([1, 2, 3, 4, 5, 6]));
        assert_eq!(mac_de(&[1, 2, 3, 4, 5]), None);
        assert_eq!(mac_de(&[0; 6]), None);
        assert_eq!(mac_de(&[0xFF; 6]), None);
        assert_eq!(
            mac_texto([0x3c, 0x7c, 0x3f, 1, 0xab, 3]),
            "3c:7c:3f:01:ab:03"
        );
    }

    #[test]
    fn broadcast_da_sub_rede() {
        let b = |ip, p| broadcast_dirigido(&iface(ip, p, &[]));
        assert_eq!(
            b([192, 168, 0, 20], 24),
            Some(Ipv4Addr::new(192, 168, 0, 255))
        );
        assert_eq!(b([10, 1, 2, 3], 8), Some(Ipv4Addr::new(10, 255, 255, 255)));
        assert_eq!(
            b([172, 16, 5, 9], 20),
            Some(Ipv4Addr::new(172, 16, 15, 255))
        );
        assert_eq!(b([192, 168, 0, 20], 32), None);
        assert_eq!(b([192, 168, 0, 20], 31), None);
        assert_eq!(b([192, 168, 0, 20], 0), None);
        assert_eq!(b([0, 0, 0, 0], 24), None);
    }

    #[test]
    fn mesma_lan_pela_sub_rede_e_pelo_gateway() {
        let gw_casa = [0xa4, 0x2b, 0xb0, 9, 8, 7];
        let gw_outra = [0xa4, 0x2b, 0xb0, 1, 1, 1];
        let alvo = iface([192, 168, 0, 20], 24, &gw_casa);
        assert!(mesma_lan(&iface([192, 168, 0, 33], 24, &gw_casa), &alvo));
        // Sem MAC de gateway de um dos lados vale só a sub-rede.
        assert!(mesma_lan(&iface([192, 168, 0, 33], 24, &[]), &alvo));
        // Mesma faixa, outra casa.
        assert!(!mesma_lan(&iface([192, 168, 0, 33], 24, &gw_outra), &alvo));
        assert!(!mesma_lan(&iface([192, 168, 1, 33], 24, &gw_casa), &alvo));
        // Prefixo do alvo manda: /16 junta o 192.168.1.x.
        let alvo16 = iface([192, 168, 0, 20], 16, &[]);
        assert!(mesma_lan(&iface([192, 168, 1, 33], 24, &[]), &alvo16));
        assert!(!mesma_lan(&iface([0, 0, 0, 0], 24, &[]), &alvo));
    }

    #[test]
    fn escolhe_as_placas_da_lan_do_alvo() {
        let locais = [
            iface([192, 168, 0, 33], 24, &[]),
            iface([10, 0, 0, 5], 24, &[]),
            iface([0, 0, 0, 0], 0, &[]),
        ];
        let alvo = iface([192, 168, 0, 20], 24, &[]);
        let so_lan = placas_para(&locais, Some(&alvo));
        assert_eq!(so_lan.len(), 1);
        assert_eq!(so_lan[0].ipv4, locais[0].ipv4);
        assert_eq!(placas_para(&locais, None).len(), 2);
        assert!(placas_para(&locais, Some(&iface([172, 16, 0, 1], 24, &[]))).is_empty());
    }

    #[test]
    fn ip_de_underlay_na_lan() {
        let alvo = iface([192, 168, 0, 20], 24, &[]);
        assert!(ip_na_lan("192.168.0.33".parse().unwrap(), &alvo));
        assert!(!ip_na_lan("192.168.1.33".parse().unwrap(), &alvo));
        assert!(!ip_na_lan("fd00::1".parse().unwrap(), &alvo));
        assert!(!ip_na_lan(
            "127.0.0.1".parse().unwrap(),
            &iface([127, 0, 0, 1], 8, &[])
        ));
        assert!(!ip_na_lan(
            "192.168.0.33".parse().unwrap(),
            &iface([0, 0, 0, 0], 24, &[])
        ));
    }

    #[test]
    fn prefere_cabo_e_wol_ligado() {
        let com = |mac: u8, wired: bool, wol: WolState| LanIface {
            mac: vec![2, 0, 0, 0, 0, mac],
            wired,
            wol: wol.into(),
            ..iface([192, 168, 0, 20], 24, &[])
        };
        let wifi = com(1, false, WolState::Armed);
        let cabo_off = com(2, true, WolState::Off);
        let cabo = com(3, true, WolState::Unknown);
        let lista = [wifi.clone(), cabo_off.clone(), cabo];
        assert_eq!(placa_para_acordar(&lista).unwrap().0, [2, 0, 0, 0, 0, 3]);
        assert_eq!(
            placa_para_acordar(&[wifi.clone(), cabo_off]).unwrap().0,
            [2, 0, 0, 0, 0, 2]
        );
        assert_eq!(placa_para_acordar(&[wifi]).unwrap().0, [2, 0, 0, 0, 0, 1]);
        let sem_mac = LanIface {
            mac: Vec::new(),
            ..com(4, true, WolState::Armed)
        };
        assert!(placa_para_acordar(&[sem_mac]).is_none());
    }

    #[test]
    fn um_pedido_por_mac_a_cada_10_s() {
        let mut l = Limite::default();
        let t = Instant::now();
        let (a, b) = ([1; 6], [2; 6]);
        assert!(l.liberar(a, t));
        assert!(!l.liberar(a, t + Duration::from_secs(9)));
        assert!(l.liberar(b, t + Duration::from_secs(9)));
        assert!(l.liberar(a, t + INTERVALO_POR_MAC));
    }
}
