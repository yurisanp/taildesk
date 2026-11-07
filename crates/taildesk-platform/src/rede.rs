//! Placas de rede deste computador nas LANs, para o Wake on LAN (`17-wake-on-lan.md` §4.1).
//!
//! Só placas físicas com IPv4: sem loopback, sem a da tailnet (`tailscale0`/Wintun), sem pontes
//! e placas virtuais (Docker, VMs). Cada uma vai com o MAC do gateway quando ela é a da rota
//! padrão, para o controlador separar duas casas com a mesma `192.168.0.0/24`.

use std::net::Ipv4Addr;

use taildesk_proto::control::{LanIface, WolState, limits};

/// As placas desta máquina, a da rota padrão primeiro. Lista vazia quando o sistema não diz.
pub fn placas_lan() -> Vec<LanIface> {
    let mut placas: Vec<(bool, LanIface)> = netdev::get_interfaces()
        .into_iter()
        .filter(|i| i.is_up() && !i.is_loopback() && !i.is_tun() && fisica(i))
        .filter_map(|i| {
            let mac = i.mac_addr?.octets();
            if mac == [0; 6] || mac == [0xFF; 6] {
                return None;
            }
            let rede = i.ipv4.iter().find(|n| lan_ipv4(n.addr())).copied()?;
            let gateway_mac = i
                .gateway
                .as_ref()
                .map(|g| g.mac_addr.octets())
                .filter(|m| *m != [0; 6])
                .map_or_else(Vec::new, |m| m.to_vec());
            let wired = !matches!(i.if_type, netdev::prelude::InterfaceType::Wireless80211)
                && !sem_fio(&i.name);
            Some((
                i.default,
                LanIface {
                    mac: mac.to_vec(),
                    ipv4: u32::from(rede.addr()),
                    prefix_len: u32::from(rede.prefix_len()),
                    gateway_mac,
                    wired,
                    wol: wol(&i.name).into(),
                },
            ))
        })
        .collect();
    placas.sort_by_key(|(padrao, _)| !*padrao);
    placas
        .into_iter()
        .map(|(_, p)| p)
        .take(limits::LAN_IFACES)
        .collect()
}

/// IPv4 que serve de LAN: nem a faixa da tailnet (100.64.0.0/10), nem link-local, nem loopback.
fn lan_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    let cgnat = a == 100 && (64..128).contains(&b);
    !(ip.is_unspecified() || ip.is_loopback() || ip.is_link_local() || cgnat)
}

/// Placa física (não ponte, não par virtual, não adaptador de VM).
#[cfg(target_os = "linux")]
fn fisica(i: &netdev::Interface) -> bool {
    // Placas virtuais não têm `device` no sysfs.
    std::path::Path::new("/sys/class/net")
        .join(&i.name)
        .join("device")
        .exists()
}

#[cfg(windows)]
fn fisica(i: &netdev::Interface) -> bool {
    let texto = format!(
        "{} {}",
        i.description.as_deref().unwrap_or_default(),
        i.friendly_name.as_deref().unwrap_or_default()
    )
    .to_lowercase();
    ![
        "virtual",
        "hyper-v",
        "wintun",
        "tailscale",
        "vpn",
        "loopback",
        "vethernet",
    ]
    .iter()
    .any(|v| texto.contains(v))
}

/// Wi-Fi que o tipo da placa não entregou (o nome no Linux diz).
#[cfg(target_os = "linux")]
fn sem_fio(nome: &str) -> bool {
    std::path::Path::new("/sys/class/net")
        .join(nome)
        .join("wireless")
        .exists()
}

#[cfg(windows)]
fn sem_fio(_: &str) -> bool {
    false
}

/// Se a placa acorda a máquina. No Linux, a flag de wakeup do dispositivo no sysfs (o
/// `ethtool -s <if> wol g` liga junto); ler o modo exato do `ethtool` pede `CAP_NET_ADMIN`.
#[cfg(target_os = "linux")]
fn wol(nome: &str) -> WolState {
    let caminho = std::path::Path::new("/sys/class/net")
        .join(nome)
        .join("device/power/wakeup");
    match std::fs::read_to_string(caminho).as_deref().map(str::trim) {
        Ok("enabled") => WolState::Armed,
        Ok("disabled") => WolState::Off,
        _ => WolState::Unknown,
    }
}

/// No Windows a configuração fica nas propriedades da placa (WMI `MSPower_DeviceWakeEnable`, só
/// com administrador): o controlador avisa só pelo cabo.
#[cfg(windows)]
fn wol(_: &str) -> WolState {
    WolState::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_ipv4_de_lan() {
        assert!(lan_ipv4(Ipv4Addr::new(192, 168, 0, 20)));
        assert!(lan_ipv4(Ipv4Addr::new(10, 0, 0, 5)));
        assert!(lan_ipv4(Ipv4Addr::new(100, 63, 0, 1)));
        assert!(!lan_ipv4(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(!lan_ipv4(Ipv4Addr::new(100, 127, 255, 1)));
        assert!(!lan_ipv4(Ipv4Addr::new(169, 254, 3, 4)));
        assert!(!lan_ipv4(Ipv4Addr::LOCALHOST));
        assert!(!lan_ipv4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn lista_respeita_os_limites() {
        // Sem garantia de placa física no CI: confere só o formato do que vier.
        for p in placas_lan() {
            assert_eq!(p.mac.len(), 6);
            assert!(p.gateway_mac.is_empty() || p.gateway_mac.len() == 6);
            assert!(p.prefix_len <= 32);
            assert!(lan_ipv4(Ipv4Addr::from(p.ipv4)));
        }
        assert!(placas_lan().len() <= limits::LAN_IFACES);
    }
}
