//! Laço rápido do controle de taxa do vídeo (`07-protocolo.md` §5, `14-selecao-adaptativa.md`
//! §3.3), no host, a cada ~100 ms.
//!
//! Devolve um **fator** em (0, 1] que multiplica o teto de cada monitor (a taxa do tamanho da
//! imagem, ou a que o usuário fixou). Com a tela parada o encoder gasta pouco e o QUIC fica
//! limitado pela aplicação: nem a banda nem a taxa que sai dizem quanto cabe. Por isso o fator
//! só **desce** com sinal de fila ou perda e, sem eles, **sobe** sozinho até 1:
//!
//! - fila: `rtt − rtt mínimo dos últimos 10 s` > 15 ms, ou o atraso de ida que o controlador
//!   mede (`ReceiverReport.owd_trend_us`) > 15 ms;
//! - perda nos relatórios do controlador > 2 %;
//! - relatório atrasado mais de 1 s com vídeo saindo (o stream de controle travou na fila).
//!
//! Cada corte tira 20 % (e, se a estimativa de banda do QUIC for menor, desce para 85 % dela) e
//! espera um RTT, no mínimo 200 ms, antes do próximo. A recuperação é de 5 % por segundo, pelo
//! menos 1 % do teto por segundo, e só com a fila abaixo de 5 ms. O piso é
//! [`ControleTaxa::PISO_BPS`] por monitor: degrada sem congelar.
//!
//! Sem I/O nem relógio próprio: quem chama passa os tempos.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// O que o controlador relatou por último (o `ReceiverReport` mais recente de qualquer monitor).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Relato {
    pub loss_ppm: u32,
    pub owd_trend_us: u32,
}

/// Sinais de um passo.
#[derive(Clone, Copy, Debug)]
pub struct Sinais {
    pub agora: Instant,
    /// RTT suavizado do QUIC.
    pub rtt: Duration,
    /// Banda que o QUIC estima (taxa de pacing do BBR), em bit/s.
    pub banda_bps: Option<u64>,
    /// Último relatório do controlador e quando chegou.
    pub relato: Option<(Instant, Relato)>,
    /// Soma dos tetos dos monitores saindo, em bit/s (`0` = nada saindo).
    pub teto_bps: u64,
    /// Monitores saindo.
    pub monitores: u32,
}

/// Por que o fator mudou no último passo (para o log e o diagnóstico).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motivo {
    Inicio,
    Fila,
    Perda,
    SemRelato,
    Recuperando,
}

impl Motivo {
    pub fn texto(self) -> &'static str {
        match self {
            Motivo::Inicio => "início",
            Motivo::Fila => "fila no caminho",
            Motivo::Perda => "perda",
            Motivo::SemRelato => "controlador sem responder",
            Motivo::Recuperando => "recuperando",
        }
    }
}

/// Estado do laço rápido de uma conexão.
#[derive(Debug)]
pub struct ControleTaxa {
    fator: f64,
    /// Mínimo do RTT por janela de 1 s, nos últimos 10 s.
    rtts: VecDeque<(Instant, Duration)>,
    ultimo_passo: Option<Instant>,
    ultimo_corte: Option<Instant>,
    motivo: Motivo,
    /// Desde quando há vídeo saindo (para não cobrar relatório antes do primeiro).
    saindo_desde: Option<Instant>,
}

impl Default for ControleTaxa {
    fn default() -> Self {
        ControleTaxa {
            fator: 1.0,
            rtts: VecDeque::new(),
            ultimo_passo: None,
            ultimo_corte: None,
            motivo: Motivo::Inicio,
            saindo_desde: None,
        }
    }
}

impl ControleTaxa {
    /// Menor taxa por monitor.
    pub const PISO_BPS: u64 = 500_000;
    const FILA_CORTE: Duration = Duration::from_millis(15);
    const FILA_LIVRE: Duration = Duration::from_millis(5);
    const PERDA_CORTE_PPM: u32 = 20_000;
    const RELATO_MAX: Duration = Duration::from_secs(1);
    const JANELA_RTT: Duration = Duration::from_secs(10);
    const CORTE: f64 = 0.8;
    const MARGEM_BANDA: f64 = 0.85;
    const SUBIDA_POR_S: f64 = 0.05;
    const SUBIDA_MIN_POR_S: f64 = 0.01;
    /// Fora da LAN o vídeo começa nesta taxa total e sobe dali (`14` §3.2: classe Média).
    const INICIO_FORA_DA_LAN_BPS: u64 = 8_000_000;
    const RTT_LAN: Duration = Duration::from_millis(5);

    pub fn fator(&self) -> f64 {
        self.fator
    }

    pub fn motivo(&self) -> Motivo {
        self.motivo
    }

    /// O teto mudou de `antes` para `depois` (o laço lento reduziu a resolução): corrige o fator
    /// para a taxa absoluta não cair junto com o teto.
    pub fn reescalar(&mut self, antes: u64, depois: u64) {
        if antes > 0 && depois > 0 {
            self.fator = (self.fator * antes as f64 / depois as f64).clamp(f64::MIN_POSITIVE, 1.0);
        }
    }

    /// Fila no caminho agora: RTT acima do mínimo recente.
    pub fn fila(&self, rtt: Duration) -> Duration {
        let min = self.rtts.iter().map(|&(_, r)| r).min().unwrap_or(rtt);
        rtt.saturating_sub(min)
    }

    fn guardar_rtt(&mut self, agora: Instant, rtt: Duration) {
        while self
            .rtts
            .front()
            .is_some_and(|&(t, _)| agora.duration_since(t) > Self::JANELA_RTT)
        {
            self.rtts.pop_front();
        }
        match self.rtts.back_mut() {
            Some((t, r)) if agora.duration_since(*t) < Duration::from_secs(1) => {
                *r = (*r).min(rtt);
            }
            _ => self.rtts.push_back((agora, rtt)),
        }
    }

    /// Um passo do laço; devolve o fator novo.
    pub fn passo(&mut self, s: &Sinais) -> f64 {
        let dt = self
            .ultimo_passo
            .map_or(Duration::ZERO, |t| s.agora.duration_since(t));
        let primeiro = self.ultimo_passo.is_none();
        self.ultimo_passo = Some(s.agora);
        self.guardar_rtt(s.agora, s.rtt);
        if s.teto_bps == 0 || s.monitores == 0 {
            self.saindo_desde = None;
            return self.fator;
        }
        let teto = s.teto_bps as f64;
        let piso = ((Self::PISO_BPS * u64::from(s.monitores)) as f64 / teto).min(1.0);
        if primeiro || self.saindo_desde.is_none() {
            self.saindo_desde = Some(s.agora);
            if primeiro && s.rtt >= Self::RTT_LAN {
                self.fator = (Self::INICIO_FORA_DA_LAN_BPS as f64 / teto).clamp(piso, 1.0);
            }
            return self.fator;
        }
        let fila = self.fila(s.rtt);
        let relato_fresco = s
            .relato
            .filter(|(t, _)| s.agora.duration_since(*t) <= Self::RELATO_MAX);
        let esperando_relato = self
            .saindo_desde
            .is_some_and(|t| s.agora.duration_since(t) > Self::RELATO_MAX);
        let congestao = if fila > Self::FILA_CORTE
            || relato_fresco.is_some_and(|(_, r)| {
                u64::from(r.owd_trend_us) > Self::FILA_CORTE.as_micros() as u64
            }) {
            Some(Motivo::Fila)
        } else if relato_fresco.is_some_and(|(_, r)| r.loss_ppm > Self::PERDA_CORTE_PPM) {
            Some(Motivo::Perda)
        } else if relato_fresco.is_none() && esperando_relato && s.relato.is_some() {
            // Só depois do primeiro relato: um controlador antigo não manda nenhum.
            Some(Motivo::SemRelato)
        } else {
            None
        };
        match congestao {
            Some(m) => {
                let espera = s.rtt.max(Duration::from_millis(200));
                if self
                    .ultimo_corte
                    .is_none_or(|t| s.agora.duration_since(t) >= espera)
                {
                    let mut f = self.fator * Self::CORTE;
                    if let Some(b) = s.banda_bps.filter(|&b| b > 0) {
                        f = f.min(b as f64 * Self::MARGEM_BANDA / teto);
                    }
                    self.fator = f.clamp(piso, 1.0);
                    self.ultimo_corte = Some(s.agora);
                    self.motivo = m;
                }
            }
            None if fila < Self::FILA_LIVRE && self.fator < 1.0 => {
                let seg = dt.as_secs_f64().min(1.0);
                let passo = (self.fator * Self::SUBIDA_POR_S).max(Self::SUBIDA_MIN_POR_S) * seg;
                self.fator = (self.fator + passo).clamp(piso, 1.0);
                self.motivo = Motivo::Recuperando;
            }
            None => {}
        }
        self.fator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    struct Rede {
        c: ControleTaxa,
        t0: Instant,
        t: Duration,
    }

    impl Rede {
        fn new() -> Self {
            Rede {
                c: ControleTaxa::default(),
                t0: Instant::now(),
                t: Duration::ZERO,
            }
        }

        /// Passos de 100 ms por `dur` com o RTT e o relato dados.
        fn por(&mut self, dur: Duration, rtt: Duration, relato: Option<Relato>) -> f64 {
            let fim = self.t + dur;
            while self.t < fim {
                self.t += 100 * MS;
                let agora = self.t0 + self.t;
                self.c.passo(&Sinais {
                    agora,
                    rtt,
                    banda_bps: None,
                    relato: relato.map(|r| (agora, r)),
                    teto_bps: 20_000_000,
                    monitores: 1,
                });
            }
            self.c.fator()
        }
    }

    #[test]
    fn lan_comeca_no_teto_e_internet_na_classe_media() {
        let mut r = Rede::new();
        assert_eq!(r.por(100 * MS, MS, None), 1.0);
        let mut r = Rede::new();
        assert!((r.por(100 * MS, 30 * MS, None) - 0.4).abs() < 1e-9);
    }

    #[test]
    fn tela_parada_nao_derruba_a_taxa() {
        // Sem fila nem perda (o caso da tela parada), o fator sobe até o teto e fica.
        let mut r = Rede::new();
        r.por(100 * MS, 30 * MS, None);
        assert_eq!(r.por(Duration::from_secs(60), 30 * MS, None), 1.0);
    }

    #[test]
    fn fila_corta_e_recupera_devagar() {
        let mut r = Rede::new();
        r.por(Duration::from_secs(2), MS, None);
        // Fila de 40 ms por 1 s: cortes de 20 % a cada 200 ms.
        let f = r.por(Duration::from_secs(1), 41 * MS, None);
        assert!(f < 0.4 && f > 0.25, "{f}");
        assert_eq!(r.c.motivo(), Motivo::Fila);
        // Fila some: sobe 5 %/s, então em 2 s ainda está longe do teto.
        let g = r.por(Duration::from_secs(2), MS, None);
        assert!(g > f && g < f * 1.15, "{f} → {g}");
        assert_eq!(r.c.motivo(), Motivo::Recuperando);
    }

    #[test]
    fn nunca_abaixo_do_piso() {
        let mut r = Rede::new();
        r.por(100 * MS, MS, None);
        // Dentro da janela de 10 s do RTT mínimo (depois disso 300 ms vira o normal).
        let f = r.por(Duration::from_secs(8), 300 * MS, None);
        assert!((f - 0.025).abs() < 1e-9, "{f}");
    }

    #[test]
    fn perda_e_atraso_do_controlador_cortam() {
        let mut r = Rede::new();
        r.por(100 * MS, MS, None);
        let perda = Relato {
            loss_ppm: 50_000,
            owd_trend_us: 0,
        };
        assert!(r.por(300 * MS, MS, Some(perda)) < 1.0);
        assert_eq!(r.c.motivo(), Motivo::Perda);

        let mut r = Rede::new();
        r.por(100 * MS, MS, None);
        let atraso = Relato {
            loss_ppm: 0,
            owd_trend_us: 30_000,
        };
        assert!(r.por(300 * MS, MS, Some(atraso)) < 1.0);
        assert_eq!(r.c.motivo(), Motivo::Fila);
    }

    #[test]
    fn relato_atrasado_corta_so_depois_do_primeiro() {
        let mut r = Rede::new();
        r.por(100 * MS, MS, None);
        // Controlador antigo: nunca relata, nada muda.
        assert_eq!(r.por(Duration::from_secs(3), MS, None), 1.0);
        // Relatou uma vez e parou.
        let t = r.t0 + r.t;
        let mut f = 1.0;
        for i in 1..=20 {
            let agora = t + i * 100 * MS;
            f = r.c.passo(&Sinais {
                agora,
                rtt: MS,
                banda_bps: None,
                relato: Some((t, Relato::default())),
                teto_bps: 20_000_000,
                monitores: 1,
            });
        }
        assert!(f < 1.0);
        assert_eq!(r.c.motivo(), Motivo::SemRelato);
    }

    #[test]
    fn banda_do_quic_limita_o_corte() {
        let mut r = Rede::new();
        r.por(100 * MS, MS, None);
        r.por(Duration::from_secs(1), MS, None);
        let agora = r.t0 + r.t + 100 * MS;
        let f = r.c.passo(&Sinais {
            agora,
            rtt: 50 * MS,
            banda_bps: Some(4_000_000),
            relato: None,
            teto_bps: 20_000_000,
            monitores: 1,
        });
        assert!((f - 0.17).abs() < 1e-9, "{f}");
    }

    #[test]
    fn reescalar_mantem_a_taxa_absoluta() {
        let mut r = Rede::new();
        // Fora da LAN começa em 8 de 20 Mbit/s.
        assert!((r.por(100 * MS, 30 * MS, None) - 0.4).abs() < 1e-9);
        // Teto caiu para 10 Mbit/s: o fator dobra e a taxa continua em 8.
        r.c.reescalar(20_000_000, 10_000_000);
        assert!((r.c.fator() - 0.8).abs() < 1e-9);
        // Nunca passa de 1.
        r.c.reescalar(10_000_000, 1_000_000);
        assert_eq!(r.c.fator(), 1.0);
        r.c.reescalar(0, 5);
        assert_eq!(r.c.fator(), 1.0);
    }
}
