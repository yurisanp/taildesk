//! Do `status` da LocalAPI para o que a janela e a bandeja mostram. Funções puras, testáveis sem
//! Tailscale nem janela.

use std::io;

use taildesk_tsapi::{PeerStatus, Status};
use taildesk_ui::{Dispositivo, Estado, EstadoTailscale};

/// Versão mínima do Tailscale do sistema (ADR-009).
pub const VERSAO_MINIMA: (u32, u32) = (1, 102);

/// Tudo o que uma leitura do Tailscale muda na interface.
#[derive(Clone, Debug)]
pub struct Visao {
    pub estado: EstadoTailscale,
    /// Uma linha para Configurações → Rede.
    pub resumo: String,
    /// Motivo técnico quando algo falhou (mensagem da LocalAPI).
    pub detalhe: String,
    pub versao: String,
    pub versao_antiga: bool,
    pub este_pc: String,
    /// Peers da tailnet, ordenados: conectáveis primeiro, depois pelo nome.
    pub dispositivos: Vec<Dispositivo>,
}

impl Visao {
    /// Antes da primeira leitura.
    pub fn verificando() -> Visao {
        Visao {
            estado: EstadoTailscale::Verificando,
            resumo: String::new(),
            detalhe: String::new(),
            versao: String::new(),
            versao_antiga: false,
            este_pc: String::new(),
            dispositivos: Vec::new(),
        }
    }

    pub fn de(leitura: &io::Result<Status>) -> Visao {
        match leitura {
            Ok(status) => de_status(status),
            Err(e) => de_erro(e),
        }
    }

    /// Linha de estado do menu da bandeja.
    pub fn linha_bandeja(&self) -> String {
        match self.estado {
            EstadoTailscale::Verificando => "Procurando o Tailscale…".into(),
            EstadoTailscale::NaoEncontrado => "Tailscale não encontrado".into(),
            EstadoTailscale::SemPermissao => "Tailscale sem permissão".into(),
            EstadoTailscale::Desconectado => "Tailscale desconectado".into(),
            EstadoTailscale::Conectado => {
                let online = self
                    .dispositivos
                    .iter()
                    .filter(|d| d.estado == Estado::Online)
                    .count();
                match online {
                    0 => format!("{} · nenhum dispositivo online", self.este_pc),
                    1 => format!("{} · 1 dispositivo online", self.este_pc),
                    n => format!("{} · {n} dispositivos online", self.este_pc),
                }
            }
        }
    }
}

fn de_erro(e: &io::Error) -> Visao {
    let estado = match e.kind() {
        io::ErrorKind::PermissionDenied => EstadoTailscale::SemPermissao,
        // Socket ou pipe ausente (não instalado ou serviço parado) ou recusando conexão.
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
            EstadoTailscale::NaoEncontrado
        }
        _ if e.to_string().contains("403") => EstadoTailscale::SemPermissao,
        _ => EstadoTailscale::NaoEncontrado,
    };
    let resumo = match estado {
        EstadoTailscale::SemPermissao => "Sem permissão para consultar o Tailscale",
        _ => "Não encontrado",
    };
    Visao {
        estado,
        resumo: resumo.into(),
        detalhe: e.to_string(),
        ..Visao::verificando()
    }
}

fn de_status(s: &Status) -> Visao {
    let versao = s.version.split('-').next().unwrap_or_default().to_owned();
    let versao_antiga = s.version_major_minor().is_some_and(|v| v < VERSAO_MINIMA);
    let este_pc = s.self_node.short_name().to_owned();
    let conectado = s.backend_state == "Running";
    let tailnet = s
        .current_tailnet
        .as_ref()
        .map(|t| t.name.as_str())
        .unwrap_or_default();
    let resumo = if conectado {
        format!("Conectado · tailnet {tailnet} · {este_pc} · versão {versao}")
    } else {
        format!("{} · versão {versao}", descrever_backend(&s.backend_state))
    };
    let mut dispositivos: Vec<Dispositivo> = if conectado {
        s.peers().map(dispositivo).collect()
    } else {
        Vec::new()
    };
    dispositivos.sort_by(|a, b| {
        ordem(a.estado)
            .cmp(&ordem(b.estado))
            .then_with(|| a.nome.to_lowercase().cmp(&b.nome.to_lowercase()))
    });
    Visao {
        estado: if conectado {
            EstadoTailscale::Conectado
        } else {
            EstadoTailscale::Desconectado
        },
        resumo,
        detalhe: if conectado {
            String::new()
        } else {
            format!("Estado do Tailscale: {}", s.backend_state)
        },
        versao,
        versao_antiga,
        este_pc,
        dispositivos,
    }
}

fn descrever_backend(estado: &str) -> &'static str {
    match estado {
        "NeedsLogin" => "Precisa de login",
        "NeedsMachineAuth" => "Esperando aprovação do administrador da tailnet",
        "Stopped" => "Desligado",
        "Starting" => "Iniciando",
        _ => "Desconectado",
    }
}

fn ordem(e: Estado) -> u8 {
    match e {
        Estado::Online | Estado::Relay => 0,
        Estado::SemApp => 1,
        Estado::Offline => 2,
        Estado::NaoSuportado => 3,
    }
}

/// Nome do sistema como a pessoa reconhece.
pub fn nome_so(os: &str) -> String {
    match os.to_ascii_lowercase().as_str() {
        "windows" => "Windows".into(),
        "linux" => "Linux".into(),
        "macos" | "darwin" => "macOS".into(),
        "ios" => "iOS".into(),
        "android" => "Android".into(),
        "" => "Sistema desconhecido".into(),
        _ => os.into(),
    }
}

fn suportado(os: &str) -> bool {
    matches!(os.to_ascii_lowercase().as_str(), "windows" | "linux")
}

/// "2026-10-14T21:03:00Z" → "14/10/2026"; ano 1 (nunca visto) ou vazio → "—".
pub fn data_curta(rfc3339: &str) -> String {
    let data = rfc3339.get(..10).unwrap_or_default();
    let partes: Vec<&str> = data.split('-').collect();
    match partes.as_slice() {
        [ano, mes, dia] if *ano != "0001" && ano.len() == 4 => format!("{dia}/{mes}/{ano}"),
        _ => "—".into(),
    }
}

fn dispositivo(p: &PeerStatus) -> Dispositivo {
    let estado = if !suportado(&p.os) {
        Estado::NaoSuportado
    } else if p.online {
        // Direto ou relay só se sabe ao conectar (o CurAddr fica vazio sem tráfego recente).
        Estado::Online
    } else {
        Estado::Offline
    };
    Dispositivo {
        nome: p.short_name().into(),
        so: nome_so(&p.os).into(),
        ip: p
            .tailscale_ips
            .iter()
            .find(|ip| ip.is_ipv4())
            .or(p.tailscale_ips.first())
            .map(|ip| ip.to_string())
            .unwrap_or_default()
            .into(),
        dns: p.dns_name.trim_end_matches('.').into(),
        estado,
        favorito: false,
        ultimo_acesso: if p.online {
            "agora".into()
        } else {
            data_curta(&p.last_seen).into()
        },
        teclado: "—".into(),
    }
}

/// Filtra pela busca (nome, sistema, IP ou nome MagicDNS), sem diferenciar maiúsculas.
pub fn filtrar(todos: &[Dispositivo], busca: &str) -> Vec<Dispositivo> {
    let busca = busca.trim().to_lowercase();
    todos
        .iter()
        .filter(|d| {
            busca.is_empty()
                || [&d.nome, &d.so, &d.ip, &d.dns]
                    .iter()
                    .any(|c| c.to_lowercase().contains(&busca))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(nome: &str, os: &str, ip: &str, online: bool) -> PeerStatus {
        PeerStatus {
            host_name: nome.into(),
            dns_name: format!("{nome}.tail1234.ts.net."),
            os: os.into(),
            tailscale_ips: vec![ip.parse().unwrap()],
            online,
            last_seen: "2026-10-14T21:03:00Z".into(),
            ..Default::default()
        }
    }

    fn status(estado: &str, versao: &str) -> Status {
        let mut s = Status {
            version: versao.into(),
            backend_state: estado.into(),
            ..Default::default()
        };
        s.self_node = peer("yuri-notebook", "linux", "100.64.0.1", true);
        s.peer = Some(
            [
                ("a", peer("servidor", "linux", "100.64.0.3", false)),
                ("b", peer("yurisp", "windows", "100.65.180.6", true)),
                ("c", peer("celular", "android", "100.64.0.9", true)),
                ("d", peer("Desktop-casa", "windows", "100.64.0.2", true)),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        );
        s
    }

    #[test]
    fn conectado_lista_e_ordena_peers() {
        let v = Visao::de(&Ok(status("Running", "1.104.1-tabc")));
        assert_eq!(v.estado, EstadoTailscale::Conectado);
        assert_eq!(v.versao, "1.104.1");
        assert!(!v.versao_antiga);
        assert_eq!(v.este_pc, "yuri-notebook");
        let nomes: Vec<&str> = v.dispositivos.iter().map(|d| d.nome.as_str()).collect();
        assert_eq!(nomes, ["Desktop-casa", "yurisp", "servidor", "celular"]);
        assert_eq!(v.dispositivos[0].dns, "Desktop-casa.tail1234.ts.net");
        assert_eq!(v.dispositivos[2].estado, Estado::Offline);
        assert_eq!(v.dispositivos[2].ultimo_acesso, "14/10/2026");
        assert_eq!(v.dispositivos[3].estado, Estado::NaoSuportado);
        assert_eq!(v.linha_bandeja(), "yuri-notebook · 2 dispositivos online");
    }

    #[test]
    fn desconectado_nao_lista_peers() {
        let v = Visao::de(&Ok(status("NeedsLogin", "1.104.1")));
        assert_eq!(v.estado, EstadoTailscale::Desconectado);
        assert!(v.dispositivos.is_empty());
        assert!(v.resumo.starts_with("Precisa de login"));
    }

    #[test]
    fn versao_antiga_avisa() {
        let v = Visao::de(&Ok(status("Running", "1.100.3")));
        assert!(v.versao_antiga);
        let v = Visao::de(&Ok(status("Running", "1.102.0")));
        assert!(!v.versao_antiga);
    }

    #[test]
    fn erros_viram_estados() {
        let nao = Visao::de(&Err(io::Error::new(io::ErrorKind::NotFound, "sem socket")));
        assert_eq!(nao.estado, EstadoTailscale::NaoEncontrado);
        let perm = Visao::de(&Err(io::Error::new(io::ErrorKind::PermissionDenied, "x")));
        assert_eq!(perm.estado, EstadoTailscale::SemPermissao);
        assert_eq!(perm.linha_bandeja(), "Tailscale sem permissão");
    }

    #[test]
    fn busca_filtra_por_nome_so_e_ip() {
        let v = Visao::de(&Ok(status("Running", "1.104.1")));
        assert_eq!(filtrar(&v.dispositivos, "").len(), 4);
        assert_eq!(filtrar(&v.dispositivos, "DESKTOP").len(), 1);
        assert_eq!(filtrar(&v.dispositivos, "windows").len(), 2);
        assert_eq!(filtrar(&v.dispositivos, "100.65").len(), 1);
        assert!(filtrar(&v.dispositivos, "nada").is_empty());
    }

    #[test]
    fn datas() {
        assert_eq!(data_curta("2026-10-14T21:03:00Z"), "14/10/2026");
        assert_eq!(data_curta("0001-01-01T00:00:00Z"), "—");
        assert_eq!(data_curta(""), "—");
    }
}
