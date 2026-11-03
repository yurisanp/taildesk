//! Lista de dispositivos do celular (MF-10..14, mockup 04).
//!
//! Fontes, em ordem (MF-11): o desktop pareado (`PeerList`), favoritos e recentes guardados no
//! celular, e endereços digitados. A lista é só sugestão: quem é o host vem da conexão. Favoritos
//! no topo, depois recentes, depois o resto (online antes de offline); busca por nome, nome
//! MagicDNS ou IP. Guardada em texto simples (uma linha por dispositivo) no armazenamento
//! privado do app, sem segredo nenhum (as impressões digitais não são segredo).

use std::net::IpAddr;

use taildesk_proto::control::{PeerEntry, TaildeskPresence};

/// Estado de um dispositivo na lista.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Situacao {
    /// Online e o TailDesk respondeu (botão Conectar).
    ComTaildesk,
    /// Online, mas sem TailDesk aceitando conexões.
    SemTaildesk,
    /// Online, sem saber do TailDesk.
    Online,
    Offline,
    Desconhecida,
}

/// Um dispositivo conhecido.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dispositivo {
    pub nome: String,
    pub dns: String,
    pub ip: IpAddr,
    /// `"windows"`, `"linux"`…
    pub os: String,
    pub situacao: Situacao,
    pub favorito: bool,
    /// Último uso, em ms desde 1970 (0 = nunca).
    pub usado_ms: i64,
    /// SHA-256 do certificado fixado (pareamento ou primeiro contato).
    pub fingerprint: Option<[u8; 32]>,
    /// É um desktop pareado (fonte da lista).
    pub pareado: bool,
    /// Nome do desktop que informou este dispositivo ("via desktop-casa").
    pub via: String,
}

impl Dispositivo {
    pub fn novo(nome: &str, ip: IpAddr) -> Dispositivo {
        Dispositivo {
            nome: nome.to_owned(),
            dns: String::new(),
            ip,
            os: String::new(),
            situacao: Situacao::Desconhecida,
            favorito: false,
            usado_ms: 0,
            fingerprint: None,
            pareado: false,
            via: String::new(),
        }
    }

    /// Pode tentar conectar (o probe ou a lista disse que tem TailDesk, ou ainda não se sabe).
    pub fn conectavel(&self) -> bool {
        matches!(
            self.situacao,
            Situacao::ComTaildesk | Situacao::Online | Situacao::Desconhecida
        )
    }
}

/// Grupos da tela (com o título de cada um).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grupo {
    Favoritos,
    Recentes,
    NaTailnet,
}

/// Todos os dispositivos conhecidos.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lista {
    itens: Vec<Dispositivo>,
}

fn hex(fp: &[u8; 32]) -> String {
    fp.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut o = [0u8; 32];
    for (i, b) in o.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(o)
}

fn limpo(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(253).collect()
}

impl Lista {
    pub fn itens(&self) -> &[Dispositivo] {
        &self.itens
    }

    pub fn por_ip(&self, ip: IpAddr) -> Option<&Dispositivo> {
        self.itens.iter().find(|d| d.ip == ip)
    }

    fn por_ip_mut(&mut self, ip: IpAddr) -> Option<&mut Dispositivo> {
        self.itens.iter_mut().find(|d| d.ip == ip)
    }

    /// Acrescenta ou atualiza (nome digitado, QR).
    pub fn guardar(&mut self, d: Dispositivo) -> &mut Dispositivo {
        let i = match self.itens.iter().position(|x| x.ip == d.ip) {
            Some(i) => {
                let x = &mut self.itens[i];
                x.nome = d.nome;
                if !d.dns.is_empty() {
                    x.dns = d.dns;
                }
                if d.fingerprint.is_some() {
                    x.fingerprint = d.fingerprint;
                }
                x.pareado |= d.pareado;
                x.favorito |= d.favorito;
                i
            }
            None => {
                self.itens.push(d);
                self.itens.len() - 1
            }
        };
        &mut self.itens[i]
    }

    /// Desktop pareado pelo QR: vira favorito e fonte da lista (MF-21).
    pub fn pareou(&mut self, nome: &str, ip: IpAddr, fingerprint: [u8; 32]) {
        let mut d = Dispositivo::novo(nome, ip);
        d.fingerprint = Some(fingerprint);
        d.pareado = true;
        d.favorito = true;
        d.situacao = Situacao::ComTaildesk;
        self.guardar(d);
    }

    /// `PeerList` do desktop `via`: atualiza o que já existe e acrescenta o resto. Os que o
    /// desktop não citou mais e não são favoritos nem recentes saem.
    pub fn mesclar(&mut self, via: &str, entradas: &[PeerEntry], meu_ip: Option<IpAddr>) {
        let mut vistos = Vec::with_capacity(entradas.len());
        for e in entradas {
            let Ok(ip) = e.ip.parse::<IpAddr>() else {
                continue;
            };
            // O próprio celular aparece na lista do desktop.
            if Some(ip) == meu_ip {
                continue;
            }
            vistos.push(ip);
            let situacao = match (e.online, e.taildesk()) {
                (false, _) => Situacao::Offline,
                (true, TaildeskPresence::Yes) => Situacao::ComTaildesk,
                (true, TaildeskPresence::No) => Situacao::SemTaildesk,
                (true, TaildeskPresence::Unknown) => Situacao::Online,
            };
            let d = match self.por_ip_mut(ip) {
                Some(d) => d,
                None => {
                    self.itens.push(Dispositivo::novo(&e.name, ip));
                    self.itens.last_mut().expect("acabou de entrar")
                }
            };
            d.nome = limpo(&e.name);
            d.dns = limpo(&e.dns_name);
            d.os = limpo(&e.os);
            // Probe recente diz mais que a lista: só rebaixa para offline.
            if !(d.situacao == Situacao::ComTaildesk && situacao == Situacao::Online) {
                d.situacao = situacao;
            }
            if !d.pareado {
                d.via = limpo(via);
            }
        }
        self.itens.retain(|d| {
            d.favorito || d.pareado || d.usado_ms > 0 || d.via != via || vistos.contains(&d.ip)
        });
    }

    /// Resultado do probe leve na porta do TailDesk (MF-12).
    pub fn sondado(&mut self, ip: IpAddr, respondeu: bool) {
        if let Some(d) = self.por_ip_mut(ip) {
            d.situacao = if respondeu {
                Situacao::ComTaildesk
            } else if d.situacao == Situacao::Offline {
                Situacao::Offline
            } else {
                Situacao::SemTaildesk
            };
        }
    }

    pub fn favoritar(&mut self, ip: IpAddr, favorito: bool) {
        if let Some(d) = self.por_ip_mut(ip) {
            d.favorito = favorito;
        }
    }

    /// Conectou: entra nos recentes e fixa o certificado do primeiro contato.
    pub fn usou(&mut self, ip: IpAddr, agora_ms: i64, fingerprint: Option<[u8; 32]>) {
        if let Some(d) = self.por_ip_mut(ip) {
            d.usado_ms = agora_ms;
            if d.fingerprint.is_none() {
                d.fingerprint = fingerprint;
            }
        }
    }

    pub fn esquecer(&mut self, ip: IpAddr) {
        self.itens.retain(|d| d.ip != ip);
    }

    /// Desktops pareados (fontes da `PeerList`).
    pub fn pareados(&self) -> impl Iterator<Item = &Dispositivo> {
        self.itens.iter().filter(|d| d.pareado)
    }

    /// Grupos para a tela, já filtrados pela busca.
    pub fn agrupar(&self, busca: &str) -> Vec<(Grupo, Vec<&Dispositivo>)> {
        let busca = busca.trim().to_lowercase();
        let casa = |d: &Dispositivo| {
            busca.is_empty()
                || d.nome.to_lowercase().contains(&busca)
                || d.dns.to_lowercase().contains(&busca)
                || d.ip.to_string().contains(&busca)
        };
        let mut fav: Vec<&Dispositivo> = Vec::new();
        let mut rec: Vec<&Dispositivo> = Vec::new();
        let mut resto: Vec<&Dispositivo> = Vec::new();
        for d in self.itens.iter().filter(|d| casa(d)) {
            if d.favorito {
                fav.push(d);
            } else if d.usado_ms > 0 {
                rec.push(d);
            } else {
                resto.push(d);
            }
        }
        let por_nome = |a: &&Dispositivo, b: &&Dispositivo| {
            a.situacao
                .cmp(&b.situacao)
                .then_with(|| a.nome.to_lowercase().cmp(&b.nome.to_lowercase()))
        };
        fav.sort_by(por_nome);
        rec.sort_by_key(|d| std::cmp::Reverse(d.usado_ms));
        rec.truncate(5);
        resto.sort_by(por_nome);
        [
            (Grupo::Favoritos, fav),
            (Grupo::Recentes, rec),
            (Grupo::NaTailnet, resto),
        ]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .collect()
    }

    /// Texto para guardar: `d\t<ip>\t<nome>\t<dns>\t<os>\t<fav>\t<usado>\t<fp>\t<pareado>\t<via>`.
    pub fn para_texto(&self) -> String {
        let mut s = String::new();
        for d in &self.itens {
            s += &format!(
                "d\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                d.ip,
                limpo(&d.nome),
                limpo(&d.dns),
                limpo(&d.os),
                u8::from(d.favorito),
                d.usado_ms,
                d.fingerprint.as_ref().map(hex).unwrap_or_default(),
                u8::from(d.pareado),
                limpo(&d.via),
            );
        }
        s
    }

    /// Lê o que [`Self::para_texto`] gravou; linhas estragadas ou de versões futuras ficam de fora.
    pub fn de_texto(texto: &str) -> Lista {
        let mut l = Lista::default();
        for linha in texto.lines() {
            let c: Vec<&str> = linha.split('\t').collect();
            let ["d", ip, nome, dns, os, fav, usado, fp, pareado, via, ..] = c.as_slice() else {
                continue;
            };
            let Ok(ip) = ip.parse() else { continue };
            if l.por_ip(ip).is_some() {
                continue;
            }
            l.itens.push(Dispositivo {
                nome: (*nome).to_owned(),
                dns: (*dns).to_owned(),
                ip,
                os: (*os).to_owned(),
                situacao: Situacao::Desconhecida,
                favorito: *fav == "1",
                usado_ms: usado.parse().unwrap_or(0),
                fingerprint: unhex(fp),
                pareado: *pareado == "1",
                via: (*via).to_owned(),
            });
        }
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn entrada(nome: &str, ip: &str, online: bool, td: TaildeskPresence) -> PeerEntry {
        PeerEntry {
            name: nome.into(),
            dns_name: format!("{nome}.tail1234.ts.net"),
            ip: ip.into(),
            os: "windows".into(),
            online,
            taildesk: td.into(),
        }
    }

    #[test]
    fn pareia_mescla_e_agrupa() {
        let mut l = Lista::default();
        l.pareou("desktop-casa", ip("100.64.0.1"), [1; 32]);
        l.mesclar(
            "desktop-casa",
            &[
                entrada("pc-mae", "100.64.0.2", true, TaildeskPresence::Unknown),
                entrada("fedora-kde", "100.64.0.3", false, TaildeskPresence::Unknown),
                entrada("eu", "100.64.0.9", true, TaildeskPresence::Unknown),
                entrada("lixo", "não-é-ip", true, TaildeskPresence::Unknown),
            ],
            Some(ip("100.64.0.9")),
        );
        assert_eq!(
            l.itens().len(),
            3,
            "o próprio celular e IP inválido ficam de fora"
        );
        let g = l.agrupar("");
        assert_eq!(g[0].0, Grupo::Favoritos);
        assert_eq!(g[0].1[0].nome, "desktop-casa");
        assert_eq!(g[1].0, Grupo::NaTailnet);
        // Online antes de offline.
        assert_eq!(g[1].1[0].nome, "pc-mae");
        assert_eq!(g[1].1[0].via, "desktop-casa");
        assert_eq!(g[1].1[1].situacao, Situacao::Offline);

        // Busca por nome MagicDNS ou IP.
        assert_eq!(l.agrupar("FEDORA")[0].1.len(), 1);
        assert_eq!(l.agrupar("100.64.0.2")[0].1[0].nome, "pc-mae");
        assert!(l.agrupar("nada").is_empty());
    }

    #[test]
    fn recentes_e_lista_que_encolhe() {
        let mut l = Lista::default();
        l.mesclar(
            "casa",
            &[
                entrada("a", "100.64.0.2", true, TaildeskPresence::Yes),
                entrada("b", "100.64.0.3", true, TaildeskPresence::No),
            ],
            None,
        );
        l.usou(ip("100.64.0.2"), 1_000, Some([9; 32]));
        assert_eq!(l.agrupar("")[0].0, Grupo::Recentes);
        assert_eq!(
            l.por_ip(ip("100.64.0.2")).unwrap().fingerprint,
            Some([9; 32])
        );
        // O desktop não cita mais nenhum: o recente fica, o outro sai.
        l.mesclar("casa", &[], None);
        assert_eq!(l.itens().len(), 1);
        assert_eq!(l.itens()[0].nome, "a");
    }

    #[test]
    fn probe_atualiza_situacao() {
        let mut l = Lista::default();
        l.guardar(Dispositivo::novo("srv", ip("100.64.0.5")));
        l.sondado(ip("100.64.0.5"), true);
        assert_eq!(l.itens()[0].situacao, Situacao::ComTaildesk);
        // A lista do desktop sem saber do TailDesk não apaga o que o probe viu.
        l.mesclar(
            "casa",
            &[entrada(
                "srv",
                "100.64.0.5",
                true,
                TaildeskPresence::Unknown,
            )],
            None,
        );
        assert_eq!(l.itens()[0].situacao, Situacao::ComTaildesk);
        l.sondado(ip("100.64.0.5"), false);
        assert_eq!(l.itens()[0].situacao, Situacao::SemTaildesk);
        assert!(!l.itens()[0].conectavel());
    }

    #[test]
    fn texto_ida_e_volta() {
        let mut l = Lista::default();
        l.pareou("desktop\tcasa", ip("100.64.0.1"), [0xab; 32]);
        let mut d = Dispositivo::novo("pc-mãe", ip("fd7a:115c:a1e0::2"));
        d.usado_ms = 42;
        d.via = "desktop-casa".into();
        l.guardar(d);
        let texto = l.para_texto();
        let volta = Lista::de_texto(&format!("{texto}futuro\tx\nd\tlixo\n"));
        assert_eq!(volta.itens().len(), 2);
        assert_eq!(volta.itens()[0].nome, "desktopcasa");
        assert_eq!(volta.itens()[0].fingerprint, Some([0xab; 32]));
        assert!(volta.itens()[0].pareado && volta.itens()[0].favorito);
        assert_eq!(volta.itens()[1].usado_ms, 42);
        assert_eq!(volta.pareados().count(), 1);
    }
}
