//! Medições de rede do TailDesk (`14-selecao-adaptativa.md` §1.3 e §3.2).
//!
//! - [`RttStats`]: mínimo, mediana, p95 e variação dos RTTs (handshake, `Ping`/`Pong`, aba de teste).
//! - [`ProbeReceiver`]: conta os datagramas de sondagem de cada degrau e monta o `ProbeResult`.
//! - [`estimate`]: transforma um `ProbeResult` na banda disponível.
//!
//! Sem I/O e sem relógio próprio: quem chama passa os tempos. Nada aqui aloca no caminho por
//! datagrama ([`ProbeReceiver::record`]).

use std::time::Duration;

use taildesk_proto::net::{PathKind, ProbeRequest, ProbeResult, ProbeStep, limits};
use taildesk_proto::probe::ProbePayload;

/// Quantas amostras de RTT [`RttStats`] guarda (as mais recentes).
pub const RTT_WINDOW: usize = 64;

/// Estatísticas de RTT sobre as últimas [`RTT_WINDOW`] amostras.
#[derive(Clone, Debug)]
pub struct RttStats {
    samples_us: [u32; RTT_WINDOW],
    len: usize,
    next: usize,
    last_us: Option<u32>,
    /// Variação suavizada como no RFC 3550 (×16 para não perder precisão).
    jitter_x16: u32,
}

impl Default for RttStats {
    fn default() -> Self {
        RttStats {
            samples_us: [0; RTT_WINDOW],
            len: 0,
            next: 0,
            last_us: None,
            jitter_x16: 0,
        }
    }
}

impl RttStats {
    pub fn push(&mut self, rtt: Duration) {
        let us = u32::try_from(rtt.as_micros()).unwrap_or(u32::MAX);
        self.samples_us[self.next] = us;
        self.next = (self.next + 1) % RTT_WINDOW;
        self.len = (self.len + 1).min(RTT_WINDOW);
        if let Some(last) = self.last_us {
            let d = us.abs_diff(last);
            // J += (|D| - J) / 16, guardando J×16.
            self.jitter_x16 = (self.jitter_x16 - self.jitter_x16 / 16).saturating_add(d);
        }
        self.last_us = Some(us);
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn min(&self) -> Option<Duration> {
        self.percentile(0)
    }

    pub fn median(&self) -> Option<Duration> {
        self.percentile(50)
    }

    pub fn p95(&self) -> Option<Duration> {
        self.percentile(95)
    }

    /// Variação suavizada entre amostras consecutivas.
    pub fn jitter(&self) -> Duration {
        Duration::from_micros((self.jitter_x16 / 16).into())
    }

    /// Percentil pelo método do posto mais próximo (`pct` em 0..=100).
    pub fn percentile(&self, pct: u8) -> Option<Duration> {
        if self.len == 0 {
            return None;
        }
        let mut sorted = self.samples_us;
        let s = &mut sorted[..self.len];
        s.sort_unstable();
        let rank = (usize::from(pct.min(100)) * self.len).div_ceil(100);
        Some(Duration::from_micros(s[rank.saturating_sub(1)].into()))
    }
}

/// Degraus de taxa da sondagem inicial, em kbit/s. No relay a sondagem para em 20 Mbit/s para
/// não abusar dos servidores DERP, que são compartilhados.
pub fn plan_steps(path: PathKind) -> &'static [u32] {
    match path {
        PathKind::Relayed => &[2_000, 5_000, 10_000, 20_000],
        _ => &[5_000, 20_000, 50_000, 100_000, 200_000],
    }
}

/// Quantos datagramas de `datagram_len` bytes cabem em `step_ms` a `kbps`.
pub fn step_packets(kbps: u32, step_ms: u32, datagram_len: usize) -> u16 {
    let bits = u64::from(kbps) * u64::from(step_ms);
    let per_pkt = (datagram_len.max(1) as u64) * 8;
    u16::try_from(bits.div_ceil(per_pkt))
        .unwrap_or(u16::MAX)
        .max(1)
}

#[derive(Clone, Copy, Debug, Default)]
struct StepAcc {
    expected: u16,
    pkts: u32,
    bytes_after_first: u64,
    first_us: Option<u32>,
    last_us: u32,
    owd_sum: i64,
}

/// Lado que recebe a sondagem: acumula por degrau e devolve o [`ProbeResult`].
#[derive(Clone, Debug)]
pub struct ProbeReceiver {
    probe_id: u32,
    steps_kbps: [u32; limits::PROBE_STEPS],
    n_steps: usize,
    acc: [StepAcc; limits::PROBE_STEPS],
    base_owd: Option<u32>,
    min_rel_owd: i64,
}

impl ProbeReceiver {
    /// `None` se o pedido estiver fora dos limites.
    pub fn new(req: &ProbeRequest) -> Option<ProbeReceiver> {
        req.validate().ok()?;
        let mut steps_kbps = [0; limits::PROBE_STEPS];
        steps_kbps[..req.steps_kbps.len()].copy_from_slice(&req.steps_kbps);
        Some(ProbeReceiver {
            probe_id: req.probe_id,
            steps_kbps,
            n_steps: req.steps_kbps.len(),
            acc: [StepAcc::default(); limits::PROBE_STEPS],
            base_owd: None,
            min_rel_owd: i64::MAX,
        })
    }

    /// Registra um datagrama de sondagem que chegou em `arrival_us` (relógio local, µs com wrap).
    /// `datagram_len` é o tamanho total do datagrama. Ignora sondagens e degraus que não são deste pedido.
    pub fn record(&mut self, p: &ProbePayload, datagram_len: usize, arrival_us: u32) {
        let step = usize::from(p.step);
        if p.probe_id != self.probe_id || step >= self.n_steps {
            return;
        }
        // Atraso de ida relativo ao primeiro datagrama: o desvio entre os relógios se cancela.
        let owd = arrival_us.wrapping_sub(p.send_us);
        let base = *self.base_owd.get_or_insert(owd);
        let rel = i64::from(owd.wrapping_sub(base) as i32);
        self.min_rel_owd = self.min_rel_owd.min(rel);

        let a = &mut self.acc[step];
        a.expected = a.expected.max(p.step_pkts);
        a.pkts += 1;
        a.owd_sum += rel;
        match a.first_us {
            None => a.first_us = Some(arrival_us),
            Some(_) => a.bytes_after_first += datagram_len as u64,
        }
        a.last_us = arrival_us;
    }

    pub fn finish(&self) -> ProbeResult {
        let steps = self.acc[..self.n_steps]
            .iter()
            .zip(&self.steps_kbps)
            .map(|(a, &sent_kbps)| {
                let span_us = a.first_us.map_or(0, |f| a.last_us.wrapping_sub(f));
                let recv_kbps = if a.pkts >= 2 && span_us > 0 {
                    // bits / µs = Mbit/s; ×1000 = kbit/s.
                    u32::try_from(a.bytes_after_first * 8 * 1000 / u64::from(span_us))
                        .unwrap_or(u32::MAX)
                } else {
                    0
                };
                let expected = u32::from(a.expected).max(1);
                let got = a.pkts.min(expected);
                let loss_ppm = if a.pkts == 0 {
                    1_000_000
                } else {
                    (expected - got) * 1_000_000 / expected
                };
                let owd_delta_us = if a.pkts == 0 {
                    0
                } else {
                    let mean = a.owd_sum / i64::from(a.pkts);
                    u32::try_from((mean - self.min_rel_owd).max(0)).unwrap_or(u32::MAX)
                };
                ProbeStep {
                    sent_kbps,
                    recv_kbps,
                    loss_ppm,
                    owd_delta_us,
                }
            })
            .collect();
        ProbeResult {
            probe_id: self.probe_id,
            steps,
        }
    }
}

/// Banda estimada a partir de uma sondagem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Estimate {
    pub kbps: u32,
    /// `true` se algum degrau saturou o caminho (a banda é a medida); `false` se todos passaram
    /// (a banda real é pelo menos `kbps`).
    pub saturated: bool,
    /// Perda no último degrau bom (ou no primeiro, se nenhum foi bom).
    pub loss_ppm: u32,
}

/// Um degrau "passou" se a perda ficou abaixo de 1% e a fila não cresceu mais de 5 ms.
pub const STEP_MAX_LOSS_PPM: u32 = 10_000;
pub const STEP_MAX_OWD_DELTA_US: u32 = 5_000;

/// Banda disponível: a maior taxa recebida até o primeiro degrau que saturou, incluindo a taxa
/// que chegou nele (em saturação, a taxa de chegada é a do gargalo).
pub fn estimate(result: &ProbeResult) -> Option<Estimate> {
    let first = result.steps.first()?;
    let mut best = 0;
    let mut loss_ppm = first.loss_ppm;
    for s in &result.steps {
        best = best.max(s.recv_kbps);
        if s.loss_ppm >= STEP_MAX_LOSS_PPM || s.owd_delta_us >= STEP_MAX_OWD_DELTA_US {
            return Some(Estimate {
                kbps: best,
                saturated: true,
                loss_ppm,
            });
        }
        loss_ppm = s.loss_ppm;
    }
    Some(Estimate {
        kbps: best,
        saturated: false,
        loss_ppm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtt_percentiles_and_jitter() {
        let mut s = RttStats::default();
        assert_eq!(s.median(), None);
        for ms in 1..=20u64 {
            s.push(Duration::from_millis(ms));
        }
        assert_eq!(s.min(), Some(Duration::from_millis(1)));
        assert_eq!(s.median(), Some(Duration::from_millis(10)));
        assert_eq!(s.p95(), Some(Duration::from_millis(19)));
        assert!(s.jitter() > Duration::ZERO);

        let mut steady = RttStats::default();
        for _ in 0..200 {
            steady.push(Duration::from_millis(30));
        }
        assert_eq!(steady.len(), RTT_WINDOW);
        assert_eq!(steady.jitter(), Duration::ZERO);
        assert_eq!(steady.p95(), Some(Duration::from_millis(30)));
    }

    #[test]
    fn step_packets_rounds_up() {
        // 20 Mbit/s por 100 ms = 250 000 bytes ≈ 209 datagramas de 1200 B.
        assert_eq!(step_packets(20_000, 100, 1200), 209);
        assert_eq!(step_packets(1, 1, 1200), 1);
        assert_eq!(step_packets(u32::MAX, 500, 1), u16::MAX);
    }

    #[test]
    fn relayed_probe_stays_polite() {
        assert!(plan_steps(PathKind::Relayed).iter().all(|&k| k <= 20_000));
        assert!(plan_steps(PathKind::DirectLan).contains(&200_000));
    }

    /// Simula um gargalo de `cap_kbps` com fila: acima dele, os datagramas atrasam e alguns se perdem.
    fn simulate(steps: &[u32], cap_kbps: u32, base_owd_us: u32) -> ProbeResult {
        let req = ProbeRequest {
            probe_id: 7,
            steps_kbps: steps.to_vec(),
            step_ms: 100,
        };
        let mut rx = ProbeReceiver::new(&req).unwrap();
        let len = 1200usize;
        let mut send_us: u32 = 1_000_000;
        let mut link_free_us: u64 = 0;
        for (i, &kbps) in steps.iter().enumerate() {
            let n = step_packets(kbps, 100, len);
            let gap_us = 100_000 / u32::from(n);
            let tx_us = (len as u64 * 8 * 1000).div_ceil(u64::from(cap_kbps));
            for seq in 0..n {
                let t = u64::from(send_us);
                let start = link_free_us.max(t);
                // Fila de até 50 ms; acima disso, descarta.
                if start - t <= 50_000 {
                    link_free_us = start + tx_us;
                    let arrival = (link_free_us + u64::from(base_owd_us)) as u32;
                    let p = ProbePayload {
                        probe_id: 7,
                        step: i as u8,
                        seq,
                        step_pkts: n,
                        send_us,
                    };
                    rx.record(&p, len, arrival);
                }
                send_us += gap_us;
            }
        }
        rx.finish()
    }

    #[test]
    fn finds_bottleneck_of_a_30_mbit_link() {
        let r = simulate(&[5_000, 20_000, 50_000, 100_000], 30_000, 20_000);
        assert_eq!(r.steps.len(), 4);
        assert!(r.steps[0].loss_ppm == 0 && r.steps[0].owd_delta_us < 1_000);
        let e = estimate(&r).unwrap();
        assert!(e.saturated);
        assert!(
            (25_000..=32_000).contains(&e.kbps),
            "estimativa {} kbit/s",
            e.kbps
        );
    }

    #[test]
    fn fast_link_is_not_saturated() {
        let r = simulate(plan_steps(PathKind::DirectLan), 1_000_000, 500);
        let e = estimate(&r).unwrap();
        assert!(!e.saturated);
        assert!(e.kbps >= 180_000, "estimativa {} kbit/s", e.kbps);
    }

    #[test]
    fn missing_step_counts_as_full_loss() {
        let req = ProbeRequest {
            probe_id: 1,
            steps_kbps: vec![5_000, 20_000],
            step_ms: 100,
        };
        let rx = ProbeReceiver::new(&req).unwrap();
        let r = rx.finish();
        assert_eq!(r.steps[1].loss_ppm, 1_000_000);
        assert!(estimate(&r).unwrap().saturated);
        assert_eq!(estimate(&ProbeResult::default()), None);
    }

    #[test]
    fn ignores_other_probes_and_steps() {
        let req = ProbeRequest {
            probe_id: 1,
            steps_kbps: vec![5_000],
            step_ms: 100,
        };
        let mut rx = ProbeReceiver::new(&req).unwrap();
        let p = ProbePayload {
            probe_id: 2,
            step: 0,
            seq: 0,
            step_pkts: 1,
            send_us: 0,
        };
        rx.record(&p, 1200, 10);
        rx.record(
            &ProbePayload {
                probe_id: 1,
                step: 3,
                ..p
            },
            1200,
            10,
        );
        assert_eq!(rx.finish().steps[0].loss_ppm, 1_000_000);
        assert!(
            ProbeReceiver::new(&ProbeRequest {
                step_ms: 10_000,
                ..req
            })
            .is_none()
        );
    }
}
