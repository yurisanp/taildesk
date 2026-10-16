//! Indicador de qualidade da conexão na barra da sessão (`12-levantamento-ux.md` §5.6): um nível
//! (Excelente, Boa, Regular, Ruim), o resumo "12 ms · direto · LAN" com a classe de rede de
//! `14-selecao-adaptativa.md` §2 e, no painel que abre ao clicar, os números e o porquê.
//!
//! Tudo aqui é lógica pura sobre uma [`Medida`] por segundo, para testar sem sessão.

use std::time::Duration;

use taildesk_media::adapt::AdaptConfig;
use taildesk_overlay::{PathInfo, PathKind};
use taildesk_proto::net::NetClass;

/// Nível mostrado pelas barras.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Nivel {
    /// Ainda sem estatísticas do host.
    Medindo,
    Ruim,
    Regular,
    Boa,
    Excelente,
}

/// O que se sabe da conexão num instante.
#[derive(Clone, Debug, Default)]
pub struct Medida {
    pub rtt: Duration,
    /// Variação do RTT entre as últimas amostras.
    pub variacao: Duration,
    /// Perda host → aqui, em partes por milhão (do QUIC do host).
    pub perda_ppm: u32,
    /// Banda estimada pelo QUIC do host (kbit/s; `0` = sem estimativa).
    pub banda_kbps: u32,
    /// Taxa do vídeo chegando agora (kbit/s).
    pub video_kbps: u32,
    pub rota: Option<PathInfo>,
    /// Quadros que se perderam e pediram quadro-chave nos últimos segundos.
    pub quadros_perdidos: u64,
    /// Quadros por segundo enviados pelo host e exibidos aqui.
    pub fps_enviados: f64,
    pub fps_exibidos: f64,
    /// Já chegou `VideoStats` do host.
    pub com_host: bool,
}

/// Resultado para a interface.
#[derive(Clone, Debug, PartialEq)]
pub struct Avaliacao {
    pub nivel: Nivel,
    pub classe: Option<NetClass>,
    /// "12 ms · direto · LAN".
    pub resumo: String,
    /// Uma frase com o motivo do nível.
    pub frase: String,
}

fn caminho(r: Option<&PathInfo>) -> PathKind {
    r.map_or(PathKind::Unknown, |r| r.kind)
}

/// Nome curto da classe (`14-selecao-adaptativa.md` §7.4).
pub fn nome_classe(c: NetClass) -> &'static str {
    match c {
        NetClass::Lan => "LAN",
        NetClass::Fast => "Rápida",
        NetClass::Medium => "Média",
        NetClass::Slow => "Lenta",
        NetClass::Minimal => "Mínima",
    }
}

/// O que cada classe quer dizer, para o painel.
pub fn descricao_classe(c: NetClass) -> &'static str {
    match c {
        NetClass::Lan => "mesma rede, muita banda",
        NetClass::Fast => "internet rápida e perto",
        NetClass::Medium => "internet comum",
        NetClass::Slow => "rede lenta: menos quadros por segundo",
        NetClass::Minimal => "rede muito ruim: imagem reduzida",
    }
}

/// Rota em uma frase ("Direta pela rede local", "Via relay do Tailscale (sao)").
pub fn texto_rota(r: Option<&PathInfo>) -> String {
    let Some(r) = r else {
        return "medindo…".into();
    };
    match r.kind {
        PathKind::DirectLan => "direta · mesma rede local".into(),
        PathKind::DirectWan => "direta pela internet".into(),
        PathKind::Relayed => match &r.derp_region {
            Some(regiao) => format!("via relay do Tailscale ({regiao})"),
            None => "via relay do Tailscale".into(),
        },
        PathKind::Unknown => "desconhecida".into(),
    }
}

fn rota_curta(r: Option<&PathInfo>) -> &'static str {
    match r.map(|r| r.kind) {
        Some(PathKind::DirectLan | PathKind::DirectWan) => "direto",
        Some(PathKind::Relayed) => "relay",
        _ => "medindo rota",
    }
}

fn pct(ppm: u32) -> String {
    format!("{:.1}", f64::from(ppm) / 10_000.0).replace('.', ",")
}

/// A banda do host sai da janela de congestionamento do QUIC, que só cresce quando há o que
/// mandar: com a tela parada (ou um vídeo bem abaixo dela) ela é só um piso da capacidade, não
/// uma medida. Só é medida quando o vídeo ocupa pelo menos metade dela.
pub fn banda_medida(m: &Medida) -> bool {
    m.banda_kbps > 0 && u64::from(m.video_kbps) * 2 >= u64::from(m.banda_kbps)
}

/// Classe de rede pela banda, RTT, perda e caminho; via relay o teto é a Média (§2,
/// modificadores). Sem banda medida ([`banda_medida`]) a rede não está limitando nada, e a
/// classe sai só do RTT, da perda e do caminho.
pub fn classe(m: &Medida) -> Option<NetClass> {
    if !m.com_host {
        return None;
    }
    let kbps = if banda_medida(m) {
        m.banda_kbps.max(m.video_kbps)
    } else {
        u32::MAX
    };
    let path = caminho(m.rota.as_ref());
    let c = AdaptConfig::default().classify(kbps, m.rtt, m.perda_ppm, path);
    Some(if path == PathKind::Relayed {
        c.min(NetClass::Medium)
    } else {
        c
    })
}

/// Nível, classe e textos de uma medida.
pub fn avaliar(m: &Medida) -> Avaliacao {
    let ms = m.rtt.as_millis();
    if !m.com_host {
        return Avaliacao {
            nivel: Nivel::Medindo,
            classe: None,
            resumo: "Medindo…".into(),
            frase: "Medindo a conexão.".into(),
        };
    }
    let classe = classe(m);
    let relay = caminho(m.rota.as_ref()) == PathKind::Relayed;
    // Exibe bem menos do que chega: a rede (ou o decode) não acompanha.
    let travando = m.fps_enviados >= 10.0 && m.fps_exibidos < m.fps_enviados * 0.5;

    // Cada motivo derruba o nível até um teto; fica o menor.
    let mut motivos: Vec<(Nivel, String)> = Vec::new();
    if m.perda_ppm > 50_000 {
        motivos.push((Nivel::Ruim, format!("perda alta ({} %)", pct(m.perda_ppm))));
    } else if m.perda_ppm > 10_000 {
        motivos.push((Nivel::Regular, format!("perda de {} %", pct(m.perda_ppm))));
    } else if m.perda_ppm > 2_000 {
        motivos.push((
            Nivel::Boa,
            format!("um pouco de perda ({} %)", pct(m.perda_ppm)),
        ));
    }
    if ms > 150 {
        motivos.push((Nivel::Ruim, format!("latência muito alta ({ms} ms)")));
    } else if ms > 80 {
        motivos.push((Nivel::Regular, format!("latência alta ({ms} ms)")));
    } else if ms > 30 {
        motivos.push((Nivel::Boa, format!("latência de {ms} ms")));
    }
    if m.variacao > Duration::from_millis(30) {
        motivos.push((
            Nivel::Regular,
            format!("latência instável (±{} ms)", m.variacao.as_millis()),
        ));
    }
    if travando {
        motivos.push((Nivel::Ruim, "o vídeo está chegando com atraso".into()));
    } else if m.quadros_perdidos > 0 {
        motivos.push((Nivel::Regular, "quadros de vídeo se perderam".into()));
    }
    if relay {
        let regiao = m
            .rota
            .as_ref()
            .and_then(|r| r.derp_region.clone())
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        motivos.push((
            Nivel::Boa,
            format!("passa por um relay do Tailscale{regiao}"),
        ));
    }
    if matches!(classe, Some(NetClass::Slow | NetClass::Minimal)) {
        motivos.push((Nivel::Regular, "pouca banda".into()));
    }

    let nivel = motivos
        .iter()
        .map(|(n, _)| *n)
        .min()
        .unwrap_or(Nivel::Excelente);
    let frase = if motivos.is_empty() {
        match m.rota.as_ref().map(|r| r.kind) {
            Some(PathKind::DirectLan) => {
                "Direto pela rede local, latência baixa e sem perda.".to_string()
            }
            _ => "Conexão direta, latência baixa e sem perda.".to_string(),
        }
    } else {
        let textos: Vec<&str> = motivos.iter().map(|(_, t)| t.as_str()).collect();
        let mut f = textos.join(", ");
        if let Some(primeira) = f.get(..1) {
            f = primeira.to_uppercase() + &f[1..];
        }
        f + "."
    };
    let mut resumo = format!("{ms} ms · {}", rota_curta(m.rota.as_ref()));
    if let Some(c) = classe {
        resumo += " · ";
        resumo += nome_classe(c);
    }
    Avaliacao {
        nivel,
        classe,
        resumo,
        frase,
    }
}

/// Variação do RTT: média das diferenças entre amostras seguidas, numa janela curta.
#[derive(Debug, Default)]
pub struct Variacao {
    amostras: std::collections::VecDeque<Duration>,
}

impl Variacao {
    const JANELA: usize = 8;

    pub fn medir(&mut self, rtt: Duration) -> Duration {
        if self.amostras.len() == Self::JANELA {
            self.amostras.pop_front();
        }
        self.amostras.push_back(rtt);
        let n = self.amostras.len().saturating_sub(1) as u32;
        if n == 0 {
            return Duration::ZERO;
        }
        let soma: Duration = self
            .amostras
            .iter()
            .zip(self.amostras.iter().skip(1))
            .map(|(a, b)| a.abs_diff(*b))
            .sum();
        soma / n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rota(kind: PathKind) -> Option<PathInfo> {
        Some(PathInfo {
            kind,
            derp_region: (kind == PathKind::Relayed).then(|| "sao".to_string()),
            ..PathInfo::default()
        })
    }

    fn lan() -> Medida {
        Medida {
            rtt: Duration::from_millis(2),
            banda_kbps: 400_000,
            video_kbps: 20_000,
            rota: rota(PathKind::DirectLan),
            fps_enviados: 60.0,
            fps_exibidos: 59.0,
            com_host: true,
            ..Medida::default()
        }
    }

    #[test]
    fn sem_host_esta_medindo() {
        let a = avaliar(&Medida::default());
        assert_eq!(a.nivel, Nivel::Medindo);
        assert_eq!(a.classe, None);
    }

    #[test]
    fn lan_folgada_e_excelente() {
        let a = avaliar(&lan());
        assert_eq!(a.nivel, Nivel::Excelente);
        assert_eq!(a.classe, Some(NetClass::Lan));
        assert_eq!(a.resumo, "2 ms · direto · LAN");
        assert!(a.frase.starts_with("Direto pela rede local"));
    }

    #[test]
    fn relay_fica_no_maximo_boa_e_media() {
        let a = avaliar(&Medida {
            rtt: Duration::from_millis(25),
            rota: rota(PathKind::Relayed),
            ..lan()
        });
        assert_eq!(a.nivel, Nivel::Boa);
        assert_eq!(a.classe, Some(NetClass::Medium));
        assert_eq!(a.resumo, "25 ms · relay · Média");
        assert_eq!(a.frase, "Passa por um relay do Tailscale (sao).");
    }

    #[test]
    fn perda_e_latencia_derrubam_o_nivel() {
        let a = avaliar(&Medida {
            perda_ppm: 20_000,
            ..lan()
        });
        assert_eq!(a.nivel, Nivel::Regular);
        assert!(a.frase.contains("2,0 %"), "{}", a.frase);
        let a = avaliar(&Medida {
            rtt: Duration::from_millis(200),
            ..lan()
        });
        assert_eq!(a.nivel, Nivel::Ruim);
        let a = avaliar(&Medida {
            perda_ppm: 80_000,
            ..lan()
        });
        assert_eq!(a.nivel, Nivel::Ruim);
        assert_eq!(a.classe, Some(NetClass::Minimal));
    }

    #[test]
    fn video_atrasado_e_ruim() {
        let a = avaliar(&Medida {
            fps_exibidos: 12.0,
            ..lan()
        });
        assert_eq!(a.nivel, Nivel::Ruim);
        let a = avaliar(&Medida {
            quadros_perdidos: 2,
            ..lan()
        });
        assert_eq!(a.nivel, Nivel::Regular);
    }

    #[test]
    fn tela_parada_nao_derruba_a_qualidade() {
        // Sem vídeo saindo, a janela do QUIC não cresce: 4 Mbit/s não é a capacidade da rede.
        let a = avaliar(&Medida {
            rtt: Duration::from_millis(20),
            banda_kbps: 4_000,
            video_kbps: 0,
            rota: rota(PathKind::DirectWan),
            fps_enviados: 0.0,
            fps_exibidos: 0.0,
            ..lan()
        });
        assert_eq!(a.classe, Some(NetClass::Fast));
        assert_eq!(a.nivel, Nivel::Excelente);
        // Sem estimativa nenhuma, idem.
        let a = avaliar(&Medida {
            banda_kbps: 0,
            video_kbps: 0,
            ..lan()
        });
        assert_eq!(a.classe, Some(NetClass::Lan));
    }

    #[test]
    fn video_ocupando_a_banda_mede_a_rede() {
        // O vídeo usa quase toda a janela: aí sim a banda é a da rede.
        let m = Medida {
            rtt: Duration::from_millis(20),
            banda_kbps: 5_000,
            video_kbps: 4_500,
            rota: rota(PathKind::DirectWan),
            ..lan()
        };
        assert!(banda_medida(&m));
        let a = avaliar(&m);
        assert_eq!(a.classe, Some(NetClass::Slow));
        assert_eq!(a.nivel, Nivel::Regular);
        assert!(!banda_medida(&lan()));
    }

    #[test]
    fn variacao_e_a_media_das_diferencas() {
        let mut v = Variacao::default();
        assert_eq!(v.medir(Duration::from_millis(10)), Duration::ZERO);
        assert_eq!(
            v.medir(Duration::from_millis(20)),
            Duration::from_millis(10)
        );
        assert_eq!(
            v.medir(Duration::from_millis(10)),
            Duration::from_millis(10)
        );
    }
}
