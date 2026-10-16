//! O que o controlador mede de um stream de vídeo para o `ReceiverReport`
//! (`14-selecao-adaptativa.md` §1.3): taxa, perda de fragmentos, jitter e atraso de ida.
//!
//! Lê só o header de cada datagrama, na task de rede, sem alocar. O atraso de ida usa o
//! `pts_us` do host (µs desde o início do envio) contra o relógio local: o deslocamento entre os
//! relógios é desconhecido mas constante, então só a variação conta.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use taildesk_proto::control::ReceiverReport;
use taildesk_proto::media::{Kind, MediaHeader, seq_newer};

/// Janela do menor atraso de ida (em fatias de 1 s).
const JANELA_OWD: usize = 10;

#[derive(Debug)]
pub struct Medidor {
    inicio: Instant,
    /// Quadro sendo contado: seq, fragmentos esperados e recebidos.
    atual: Option<(u16, u16, u16)>,
    bytes: u64,
    esperados: u64,
    recebidos: u64,
    quadros: u32,
    /// Atraso de ida (relativo ao primeiro) do primeiro fragmento do quadro anterior.
    owd_anterior: Option<i64>,
    owd_ref: Option<u32>,
    /// Menor atraso por fatia de 1 s.
    minimos: VecDeque<(Instant, i64)>,
    /// Soma e contagem dos atrasos no intervalo.
    owd_soma: i64,
    owd_n: u32,
    /// Jitter da RFC 3550, em µs × 16.
    jitter16: u64,
    ultimo: Instant,
}

impl Medidor {
    pub fn novo(agora: Instant) -> Self {
        Medidor {
            inicio: agora,
            atual: None,
            bytes: 0,
            esperados: 0,
            recebidos: 0,
            quadros: 0,
            owd_anterior: None,
            owd_ref: None,
            minimos: VecDeque::with_capacity(JANELA_OWD + 1),
            owd_soma: 0,
            owd_n: 0,
            jitter16: 0,
            ultimo: agora,
        }
    }

    /// Fecha o quadro em contagem: fragmentos que faltaram contam como perdidos.
    fn fechar(&mut self) {
        if let Some((_, total, got)) = self.atual.take() {
            self.esperados += u64::from(total);
            self.recebidos += u64::from(got.min(total));
            if got >= total {
                self.quadros += 1;
            }
        }
    }

    /// Um datagrama de vídeo do stream, chegando em `agora`.
    pub fn datagrama(&mut self, datagrama: &[u8], agora: Instant) {
        let Ok((h, _)) = MediaHeader::parse(datagrama) else {
            return;
        };
        if h.kind != Kind::Video || h.frag_count == 0 {
            return;
        }
        self.bytes += datagrama.len() as u64;
        match self.atual {
            Some((s, _, ref mut got)) if s == h.frame_seq => {
                *got = got.saturating_add(1);
                return;
            }
            Some((s, _, _)) if !seq_newer(h.frame_seq, s) => {
                // Atrasado: chegou, mas o quadro dele já foi contado.
                return;
            }
            Some((s, _, _)) => {
                self.fechar();
                // Quadros inteiros que sumiram entre os dois: conta um fragmento cada (o
                // tamanho deles não se sabe).
                let pulados = u64::from(h.frame_seq.wrapping_sub(s).saturating_sub(1));
                self.esperados += pulados;
            }
            None => {}
        }
        self.atual = Some((h.frame_seq, h.frag_count, 1));
        self.chegada_de_quadro(h.pts_us, agora);
    }

    fn chegada_de_quadro(&mut self, pts_us: u32, agora: Instant) {
        let local = agora.duration_since(self.inicio).as_micros() as u32;
        let d = local.wrapping_sub(pts_us);
        let r = *self.owd_ref.get_or_insert(d);
        let owd = i64::from(d.wrapping_sub(r) as i32);
        if let Some(a) = self.owd_anterior {
            let dif = (owd - a).unsigned_abs();
            // J += (|D| − J) / 16, em ponto fixo.
            self.jitter16 = self.jitter16 + dif - self.jitter16 / 16;
        }
        self.owd_anterior = Some(owd);
        self.owd_soma += owd;
        self.owd_n += 1;
        match self.minimos.back_mut() {
            Some((t, m)) if agora.duration_since(*t) < Duration::from_secs(1) => {
                *m = (*m).min(owd);
            }
            _ => {
                if self.minimos.len() == JANELA_OWD {
                    self.minimos.pop_front();
                }
                self.minimos.push_back((agora, owd));
            }
        }
    }

    /// Fecha o intervalo e monta o relatório (sem `decode_us` e `queue_frames`, que vêm do
    /// decoder).
    pub fn relatorio(&mut self, display_id: u32, agora: Instant) -> ReceiverReport {
        let seg = agora.duration_since(self.ultimo).as_secs_f64().max(1e-3);
        self.ultimo = agora;
        let perdidos = self.esperados.saturating_sub(self.recebidos);
        let loss_ppm = (perdidos * 1_000_000)
            .checked_div(self.esperados)
            .map_or(0, |p| p.min(1_000_000) as u32);
        let min = self.minimos.iter().map(|&(_, m)| m).min();
        let owd_trend_us = match (min, self.owd_n) {
            (Some(min), n) if n > 0 => {
                (self.owd_soma / i64::from(n) - min).clamp(0, i64::from(u32::MAX)) as u32
            }
            _ => 0,
        };
        let r = ReceiverReport {
            display_id,
            recv_kbps: (self.bytes as f64 * 8.0 / 1000.0 / seg) as u32,
            loss_ppm,
            jitter_us: (self.jitter16 / 16).min(u64::from(u32::MAX)) as u32,
            decode_us: 0,
            queue_frames: 0,
            owd_trend_us,
            frames: self.quadros,
        };
        self.bytes = 0;
        self.esperados = 0;
        self.recebidos = 0;
        self.quadros = 0;
        self.owd_soma = 0;
        self.owd_n = 0;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::Fragmentador;

    const MS: Duration = Duration::from_millis(1);

    /// Manda `quadros` quadros de 3 fragmentos, um a cada 16 ms, com `atraso(i)` a mais na
    /// rede, descartando os fragmentos em que `perde(i, frag)`.
    fn rodar(
        m: &mut Medidor,
        t0: Instant,
        quadros: u32,
        atraso: impl Fn(u32) -> Duration,
        perde: impl Fn(u32, u16) -> bool,
    ) -> Instant {
        let mut f = Fragmentador::novo(1);
        let au = vec![0u8; 3000];
        let mut fim = t0;
        for i in 0..quadros {
            let envio = 16 * MS * i;
            let chegada = t0 + envio + atraso(i);
            let mut frag = 0u16;
            f.fragmentar(&au, i == 0, envio.as_micros() as u32, 1100, |d| {
                if !perde(i, frag) {
                    m.datagrama(d, chegada);
                }
                frag += 1;
            });
            fim = chegada;
        }
        fim
    }

    #[test]
    fn sem_perda_nem_fila() {
        let t0 = Instant::now();
        let mut m = Medidor::novo(t0);
        let fim = rodar(&mut m, t0, 16, |_| 5 * MS, |_, _| false);
        let r = m.relatorio(7, fim + 16 * MS);
        assert_eq!(r.display_id, 7);
        assert_eq!(r.loss_ppm, 0);
        assert!(r.owd_trend_us < 100, "{}", r.owd_trend_us);
        assert!(r.jitter_us < 100, "{}", r.jitter_us);
        // O último quadro ainda está aberto; os 15 anteriores contam.
        assert_eq!(r.frames, 15);
        assert!(r.recv_kbps > 0);
    }

    #[test]
    fn fragmentos_e_quadros_perdidos() {
        let t0 = Instant::now();
        let mut m = Medidor::novo(t0);
        // Quadro 3 perde 1 de 3 fragmentos; o quadro 6 some inteiro.
        let fim = rodar(&mut m, t0, 10, |_| MS, |i, f| (i == 3 && f == 1) || i == 6);
        let r = m.relatorio(0, fim);
        // Fechados: 0..=8 menos o 6 = 8 quadros, 24 fragmentos esperados + 1 do quadro sumido;
        // perdidos 2.
        assert_eq!(r.loss_ppm, (2 * 1_000_000 / 25) as u32);
        assert_eq!(r.frames, 7);
    }

    #[test]
    fn fila_crescendo_aparece_no_atraso() {
        let t0 = Instant::now();
        let mut m = Medidor::novo(t0);
        let fim = rodar(&mut m, t0, 30, |_| 2 * MS, |_, _| false);
        m.relatorio(0, fim);
        // A fila enche: cada quadro chega 2 ms mais tarde que o anterior.
        let t1 = t0 + 16 * MS * 30;
        let mut f = Fragmentador::novo(1);
        for _ in 0..30 {
            f.fragmentar(&[0; 10], true, 0, 1100, |_| {});
        }
        let mut fim = t1;
        for i in 0..15u32 {
            let envio = 16 * MS * (30 + i);
            let chegada = t0 + envio + 2 * MS + 2 * MS * (i + 1);
            f.fragmentar(&[0; 10], false, envio.as_micros() as u32, 1100, |d| {
                m.datagrama(d, chegada)
            });
            fim = chegada;
        }
        let r = m.relatorio(0, fim);
        // Média de 2..30 ms acima do mínimo ≈ 16 ms.
        assert!(
            r.owd_trend_us > 12_000 && r.owd_trend_us < 20_000,
            "{}",
            r.owd_trend_us
        );
        assert!(r.jitter_us > 500, "{}", r.jitter_us);
    }
}
