//! Áudio do sistema (`15-audio.md`): Opus a 48 kHz estéreo em quadros de 20 ms, um quadro por
//! datagrama `kind=1`.
//!
//! - Host: [`Conversor`] leva as amostras do dispositivo (qualquer taxa e número de canais) para
//!   48 kHz estéreo; [`EnvioAudio`] junta quadros, detecta silêncio e codifica. Silêncio não gasta
//!   rede: o último quadro sai com `LAST` e o próximo som volta com `KEY` (`15` §6.2).
//! - Controlador: [`RecepcaoAudio`] decodifica, esconde perda curta com o PLC da libopus e põe as
//!   amostras numa fila sem lock que o callback de som consome por [`Reproducao`], com um
//!   colchão de [`COLCHAO_MS`] e corte de atraso acumulado.
//!
//! Nada aqui abre dispositivo: quem chama liga o `cpal` (`taildesk-platform::som`).

use std::time::Duration;

use opus::{Application, Bitrate, Channels};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use taildesk_proto::media::{Flags, HEADER_LEN, Kind, MediaHeader, seq_newer};

/// Taxa do Opus no fio.
pub const TAXA: u32 = 48_000;
pub const CANAIS: usize = 2;
/// 20 ms a 48 kHz, por canal.
pub const QUADRO: usize = 960;
pub const QUADRO_MS_X10: u32 = 200;
/// Taxa padrão (`15` §5: perfil padrão).
pub const BITRATE_KBPS: u32 = 64;
/// Silêncio digital (≈ −90 dBFS).
const LIMIAR_SILENCIO: f32 = 3.2e-5;
/// Quadros silenciosos seguidos antes de parar de enviar (100 ms).
const QUADROS_SILENCIO: u32 = 5;
/// Áudio guardado antes de começar a tocar e alvo depois de um corte.
pub const COLCHAO_MS: u32 = 60;
/// Acima disto de áudio na fila, o excesso é jogado fora (deriva de relógio, rajada).
const ATRASO_MAX_MS: u32 = 200;
/// Quadros perdidos seguidos que o PLC ainda cobre.
const PLC_MAX: u16 = 3;

/// Leva amostras intercaladas de `taxa`/`canais` para 48 kHz estéreo, por interpolação linear.
#[derive(Debug)]
pub struct Conversor {
    taxa: u32,
    canais: usize,
    /// Posição da próxima amostra de saída, em amostras de entrada (fração).
    pos: f64,
    anterior: [f32; CANAIS],
}

impl Conversor {
    pub fn novo(taxa: u32, canais: u16) -> Self {
        Conversor {
            taxa: taxa.max(1),
            canais: usize::from(canais.max(1)),
            pos: 0.0,
            anterior: [0.0; CANAIS],
        }
    }

    fn estereo(&self, quadro: &[f32]) -> [f32; CANAIS] {
        match quadro {
            [m] => [*m, *m],
            [l, r, ..] => [*l, *r],
            [] => [0.0; CANAIS],
        }
    }

    /// Converte `entrada` e acrescenta em `saida` (intercalada, estéreo).
    pub fn converter(&mut self, entrada: &[f32], saida: &mut Vec<f32>) {
        let quadros = entrada.len() / self.canais;
        if self.taxa == TAXA {
            for q in entrada.chunks_exact(self.canais) {
                saida.extend_from_slice(&self.estereo(q));
            }
            if let Some(q) = entrada.chunks_exact(self.canais).last() {
                self.anterior = self.estereo(q);
            }
            return;
        }
        let passo = f64::from(self.taxa) / f64::from(TAXA);
        // `pos` conta a partir da amostra anterior (índice -1 = `self.anterior`).
        while self.pos < quadros as f64 {
            let i = self.pos.floor();
            let f = (self.pos - i) as f32;
            let i = i as isize;
            let a = if i < 1 {
                self.anterior
            } else {
                self.estereo(&entrada[(i as usize - 1) * self.canais..][..self.canais])
            };
            let b = if i < 0 {
                self.anterior
            } else {
                self.estereo(&entrada[i as usize * self.canais..][..self.canais])
            };
            for c in 0..CANAIS {
                saida.push(a[c] + (b[c] - a[c]) * f);
            }
            self.pos += passo;
        }
        self.pos -= quadros as f64;
        if let Some(q) = entrada.chunks_exact(self.canais).last() {
            self.anterior = self.estereo(q);
        }
    }
}

/// Erro do Opus.
#[derive(Debug)]
pub struct ErroAudio(pub String);

impl std::fmt::Display for ErroAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ErroAudio {}

impl From<opus::Error> for ErroAudio {
    fn from(e: opus::Error) -> Self {
        ErroAudio(e.to_string())
    }
}

/// Contadores do envio, para o diagnóstico e o teste de "zero pacotes" (`15` §4.4).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EstatisticasAudio {
    pub quadros: u64,
    pub bytes: u64,
    pub silencio: u64,
}

/// Junta 48 kHz estéreo em quadros de 20 ms e codifica em datagramas.
pub struct EnvioAudio {
    stream_id: u8,
    encoder: opus::Encoder,
    pendente: Vec<f32>,
    pacote: Vec<u8>,
    datagrama: Vec<u8>,
    seq: u16,
    /// Amostras desde o início, para o `pts_us`.
    amostras: u64,
    /// Quadros silenciosos seguidos.
    quieto: u32,
    /// Um trecho está saindo (o próximo quadro não leva `KEY`).
    em_trecho: bool,
    pub estatisticas: EstatisticasAudio,
}

impl std::fmt::Debug for EnvioAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvioAudio")
            .field("stream_id", &self.stream_id)
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

impl EnvioAudio {
    pub fn novo(stream_id: u8, bitrate_kbps: u32) -> Result<Self, ErroAudio> {
        let mut encoder = opus::Encoder::new(TAXA, Channels::Stereo, Application::Audio)?;
        encoder.set_bitrate(Bitrate::Bits((bitrate_kbps * 1000) as i32))?;
        Ok(EnvioAudio {
            stream_id,
            encoder,
            pendente: Vec::with_capacity(QUADRO * CANAIS * 4),
            pacote: vec![0; 1500],
            datagrama: Vec::with_capacity(1500),
            seq: 0,
            amostras: 0,
            quieto: QUADROS_SILENCIO,
            em_trecho: false,
            estatisticas: EstatisticasAudio::default(),
        })
    }

    pub fn ajustar_bitrate(&mut self, kbps: u32) -> Result<(), ErroAudio> {
        self.encoder
            .set_bitrate(Bitrate::Bits((kbps * 1000) as i32))?;
        Ok(())
    }

    /// Amostras 48 kHz estéreo intercaladas. `max_datagrama` é o `max_datagram_size()` atual.
    pub fn empurrar(
        &mut self,
        amostras: &[f32],
        max_datagrama: usize,
        mut enviar: impl FnMut(&[u8]),
    ) -> Result<(), ErroAudio> {
        self.pendente.extend_from_slice(amostras);
        let n = QUADRO * CANAIS;
        let mut ini = 0;
        while self.pendente.len() - ini >= n {
            let quadro = &self.pendente[ini..ini + n];
            ini += n;
            let pts_us = (self.amostras * 1_000_000 / u64::from(TAXA)) as u32;
            self.amostras += QUADRO as u64;
            let silencioso = quadro.iter().all(|s| s.abs() < LIMIAR_SILENCIO);
            self.quieto = if silencioso { self.quieto + 1 } else { 0 };
            if self.quieto > QUADROS_SILENCIO {
                self.estatisticas.silencio += 1;
                continue;
            }
            let carga = max_datagrama
                .saturating_sub(HEADER_LEN)
                .min(self.pacote.len());
            if carga < 64 {
                continue;
            }
            let tam = self
                .encoder
                .encode_float(quadro, &mut self.pacote[..carga])?;
            let mut flags = Flags::empty();
            if !self.em_trecho {
                flags = flags.with(Flags::KEY);
            }
            self.em_trecho = self.quieto < QUADROS_SILENCIO;
            if !self.em_trecho {
                flags = flags.with(Flags::LAST);
            }
            self.datagrama.clear();
            self.datagrama.resize(HEADER_LEN, 0);
            MediaHeader {
                kind: Kind::Audio,
                flags,
                stream_id: self.stream_id,
                frame_seq: self.seq,
                frag_idx: 0,
                frag_count: 1,
                pts_us,
                fec_group: 0,
                codec_aux: 1,
            }
            .write(&mut self.datagrama);
            self.datagrama.extend_from_slice(&self.pacote[..tam]);
            self.seq = self.seq.wrapping_add(1);
            self.estatisticas.quadros += 1;
            self.estatisticas.bytes += self.datagrama.len() as u64;
            enviar(&self.datagrama);
        }
        self.pendente.drain(..ini);
        Ok(())
    }
}

/// Fila entre a decodificação (produtor) e o callback de som (consumidor).
pub fn fila(ms: u32) -> (HeapProd<f32>, HeapCons<f32>) {
    HeapRb::<f32>::new(amostras_em(ms)).split()
}

fn amostras_em(ms: u32) -> usize {
    (TAXA as usize * CANAIS * ms as usize) / 1000
}

/// Decodifica os datagramas `kind=1` de um stream para a fila de reprodução.
pub struct RecepcaoAudio {
    stream_id: u8,
    decoder: opus::Decoder,
    saida: HeapProd<f32>,
    pcm: Vec<f32>,
    ultimo: Option<u16>,
    /// Depois de um `LAST`, falta de pacotes é silêncio, não perda.
    em_trecho: bool,
    pub perdidos: u64,
    pub recebidos: u64,
}

impl std::fmt::Debug for RecepcaoAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecepcaoAudio")
            .field("stream_id", &self.stream_id)
            .field("ultimo", &self.ultimo)
            .finish_non_exhaustive()
    }
}

impl RecepcaoAudio {
    pub fn nova(stream_id: u8, saida: HeapProd<f32>) -> Result<Self, ErroAudio> {
        Ok(RecepcaoAudio {
            stream_id,
            decoder: opus::Decoder::new(TAXA, Channels::Stereo)?,
            saida,
            pcm: vec![0.0; QUADRO * CANAIS * 6],
            ultimo: None,
            em_trecho: false,
            perdidos: 0,
            recebidos: 0,
        })
    }

    fn tocar(&mut self, n: usize) {
        // Fila cheia: o callback está atrasado; descartar é melhor que travar.
        self.saida.push_slice(&self.pcm[..n * CANAIS]);
    }

    /// Um datagrama de áudio. Os de outro stream ou atrasados são ignorados.
    pub fn receber(&mut self, datagrama: &[u8]) {
        let Ok((h, carga)) = MediaHeader::parse(datagrama) else {
            return;
        };
        if h.kind != Kind::Audio || h.stream_id != self.stream_id {
            return;
        }
        if self.ultimo.is_some_and(|u| !seq_newer(h.frame_seq, u)) {
            return;
        }
        let inicio = h.flags.contains(Flags::KEY);
        if let Some(u) = self.ultimo
            && self.em_trecho
            && !inicio
        {
            let faltam = h.frame_seq.wrapping_sub(u).wrapping_sub(1);
            self.perdidos += u64::from(faltam);
            for _ in 0..faltam.min(PLC_MAX) {
                // Pacote vazio = PLC.
                // O PLC gera o tamanho do buffer: um quadro.
                match self
                    .decoder
                    .decode_float(&[], &mut self.pcm[..QUADRO * CANAIS], false)
                {
                    Ok(n) => self.tocar(n),
                    Err(_) => break,
                }
            }
        }
        if inicio {
            let _ = self.decoder.reset_state();
        }
        self.ultimo = Some(h.frame_seq);
        self.em_trecho = !h.flags.contains(Flags::LAST);
        self.recebidos += 1;
        if let Ok(n) = self.decoder.decode_float(carga, &mut self.pcm, false) {
            self.tocar(n);
        }
    }
}

/// Lado do callback de som: tira da fila com colchão e corte de atraso.
pub struct Reproducao {
    entrada: HeapCons<f32>,
    tocando: bool,
    colchao: usize,
    maximo: usize,
}

impl std::fmt::Debug for Reproducao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reproducao")
            .field("tocando", &self.tocando)
            .finish_non_exhaustive()
    }
}

impl Reproducao {
    pub fn nova(entrada: HeapCons<f32>) -> Self {
        Reproducao {
            entrada,
            tocando: false,
            colchao: amostras_em(COLCHAO_MS),
            maximo: amostras_em(ATRASO_MAX_MS),
        }
    }

    /// Áudio na fila agora.
    pub fn atraso(&self) -> Duration {
        let n = self.entrada.occupied_len() / CANAIS;
        Duration::from_micros(n as u64 * 1_000_000 / u64::from(TAXA))
    }

    /// Preenche `saida` (48 kHz estéreo intercalado); o que faltar vira silêncio.
    pub fn preencher(&mut self, saida: &mut [f32]) {
        let na_fila = self.entrada.occupied_len();
        if na_fila > self.maximo {
            self.entrada.skip(na_fila - self.colchao);
        }
        if !self.tocando && self.entrada.occupied_len() >= self.colchao {
            self.tocando = true;
        }
        let n = if self.tocando {
            self.entrada.pop_slice(saida)
        } else {
            0
        };
        if n < saida.len() {
            // Esvaziou: espera o colchão de novo.
            self.tocando = false;
            saida[n..].fill(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seno(quadros: usize, taxa: u32, canais: usize, freq: f32) -> Vec<f32> {
        (0..quadros)
            .flat_map(|i| {
                let v = (i as f32 * freq * std::f32::consts::TAU / taxa as f32).sin() * 0.5;
                std::iter::repeat_n(v, canais)
            })
            .collect()
    }

    #[test]
    fn conversor_mantem_a_duracao() {
        for (taxa, canais) in [(48_000, 2), (44_100, 2), (44_100, 1), (96_000, 6)] {
            let mut c = Conversor::novo(taxa, canais);
            let mut saida = Vec::new();
            // 1 s em blocos de 441 quadros.
            let entrada = seno(taxa as usize, taxa, usize::from(canais), 440.0);
            for b in entrada.chunks(441 * usize::from(canais)) {
                c.converter(b, &mut saida);
            }
            let quadros = saida.len() / CANAIS;
            assert!(
                quadros.abs_diff(TAXA as usize) <= 2,
                "{taxa}/{canais}: {quadros}"
            );
            assert!(saida.iter().all(|s| s.abs() <= 0.51));
        }
    }

    #[test]
    fn opus_de_ponta_a_ponta() {
        let mut envio = EnvioAudio::novo(0xF0, BITRATE_KBPS).unwrap();
        let (prod, cons) = fila(1000);
        let mut rx = RecepcaoAudio::nova(0xF0, prod).unwrap();
        let mut rep = Reproducao::nova(cons);
        let som = seno(TAXA as usize / 2, TAXA, CANAIS, 440.0);
        let mut n = 0;
        envio
            .empurrar(&som, 1200, |d| {
                let (h, _) = MediaHeader::parse(d).unwrap();
                assert_eq!(h.kind, Kind::Audio);
                assert_eq!(h.flags.contains(Flags::KEY), n == 0);
                assert!(d.len() <= 1200);
                n += 1;
                rx.receber(d);
            })
            .unwrap();
        assert_eq!(n, 25, "500 ms = 25 quadros de 20 ms");
        assert_eq!(rx.recebidos, 25);
        // A fila passou do atraso máximo e cai para o colchão: toca os últimos 60 ms.
        let mut saida = vec![0.0; amostras_em(COLCHAO_MS)];
        rep.preencher(&mut saida);
        let energia: f32 = saida.iter().map(|s| s * s).sum::<f32>() / saida.len() as f32;
        assert!(energia > 0.05, "{energia}");
    }

    #[test]
    fn silencio_nao_gasta_rede() {
        let mut envio = EnvioAudio::novo(0xF0, BITRATE_KBPS).unwrap();
        let mut flags = Vec::new();
        let empurrar = |e: &mut EnvioAudio, a: &[f32]| {
            e.empurrar(a, 1200, |d| {
                flags.push(MediaHeader::parse(d).unwrap().0.flags);
            })
            .unwrap();
        };
        {
            let mut empurrar = empurrar;
            empurrar(&mut envio, &seno(QUADRO * 3, TAXA, CANAIS, 440.0));
            // 1 s de silêncio: só os 5 primeiros quadros saem (o último com `LAST`).
            empurrar(&mut envio, &vec![0.0; TAXA as usize * CANAIS]);
            empurrar(&mut envio, &seno(QUADRO, TAXA, CANAIS, 440.0));
        }
        assert_eq!(flags.len(), 3 + 5 + 1);
        assert!(flags[0].contains(Flags::KEY));
        assert!(flags[7].contains(Flags::LAST));
        assert!(!flags[6].contains(Flags::LAST));
        assert!(flags[8].contains(Flags::KEY));
        assert_eq!(envio.estatisticas.silencio, 50 - 5);
    }

    #[test]
    fn perda_curta_vira_plc_e_trecho_novo_nao_conta() {
        let mut envio = EnvioAudio::novo(0xF0, BITRATE_KBPS).unwrap();
        let (prod, cons) = fila(2000);
        let mut rx = RecepcaoAudio::nova(0xF0, prod).unwrap();
        let mut i = 0;
        envio
            .empurrar(&seno(QUADRO * 10, TAXA, CANAIS, 440.0), 1200, |d| {
                if i != 4 && i != 5 {
                    rx.receber(d);
                }
                i += 1;
            })
            .unwrap();
        assert_eq!(rx.perdidos, 2);
        // 10 quadros na fila: 8 recebidos + 2 de PLC.
        assert_eq!(cons.occupied_len(), QUADRO * CANAIS * 10);
    }

    #[test]
    fn reproducao_com_colchao_e_corte_de_atraso() {
        let (mut prod, cons) = fila(1000);
        let mut rep = Reproducao::nova(cons);
        let mut saida = vec![1.0; 480 * CANAIS];
        // Menos que o colchão: silêncio.
        prod.push_slice(&vec![0.5; amostras_em(COLCHAO_MS) - 2]);
        rep.preencher(&mut saida);
        assert!(saida.iter().all(|&s| s == 0.0));
        prod.push_slice(&[0.5, 0.5]);
        rep.preencher(&mut saida);
        assert!(saida.iter().all(|&s| s == 0.5));
        // 500 ms acumulados: cai para o colchão.
        prod.push_slice(&vec![0.25; amostras_em(500)]);
        rep.preencher(&mut saida);
        assert!(rep.atraso() <= Duration::from_millis(u64::from(COLCHAO_MS)));
    }
}
