//! Envio de vídeo de um monitor no host: captura → I420 → encoder → datagramas.
//!
//! A captura entrega cada quadro com [`EnvioVideo::entregar`], que copia a imagem para um buffer
//! "latest-wins" e volta; uma thread por monitor codifica o quadro mais recente, corta em
//! datagramas e envia pela [`SaidaDatagramas`]. Quadro que chega enquanto o anterior codifica
//! substitui o que esperava. Nada aloca por quadro depois do primeiro.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use taildesk_codec::{OrdemRgb, QuadroRgb, TipoQuadro, VideoEncoder};

use super::fragmentos::Fragmentador;

/// Para onde vão os datagramas (a conexão QUIC, em produção).
pub trait SaidaDatagramas: Send {
    /// `max_datagram_size()` atual; `None` = o peer não aceita datagramas.
    fn max_datagrama(&self) -> Option<usize>;
    fn enviar(&mut self, datagrama: &[u8]);
    /// Bytes enviados que ainda esperam a vez de sair (a fila de datagramas do quinn).
    fn na_fila(&self) -> usize {
        0
    }
}

/// Fila que faz o envio pular o próximo quadro: 100 ms na taxa atual, no mínimo 64 KiB. Sem
/// isso, numa rede mais lenta que o encoder, a fila do quinn cresce e a imagem chega segundos
/// atrasada; pulando antes do encode, a cadeia de referências não quebra.
fn fila_maxima(bps: u32) -> usize {
    (bps as usize / 8 / 10).max(64 * 1024)
}

/// Parâmetros do envio de um monitor.
#[derive(Clone, Copy, Debug)]
pub struct ConfigEnvio {
    /// `stream_id` anunciado no `CodecSelected`.
    pub stream_id: u8,
    /// Teto de quadros por segundo; a captura pode entregar mais.
    pub fps_max: f32,
}

/// Contadores do envio, para a aba de teste de conexão e para a CLI.
#[derive(Debug, Default)]
pub struct EstatisticasEnvio {
    pub quadros: AtomicU64,
    pub chaves: AtomicU64,
    /// Quadros que o controle de taxa pulou ou que um quadro mais novo substituiu.
    pub pulados: AtomicU64,
    pub bytes: AtomicU64,
    pub datagramas: AtomicU64,
    pub erros: AtomicU64,
    /// Dos pulados, os que a fila de envio cheia segurou.
    pub fila_cheia: AtomicU64,
    /// Soma dos tempos de conversão + encode, em µs.
    pub encode_us: AtomicU64,
    /// Tamanho do último quadro codificado: largura nos 32 bits altos, altura nos baixos.
    pub tamanho: AtomicU64,
}

impl EstatisticasEnvio {
    /// (largura, altura) do último quadro codificado; (0, 0) antes do primeiro.
    pub fn tamanho(&self) -> (u32, u32) {
        let t = self.tamanho.load(Ordering::Relaxed);
        ((t >> 32) as u32, t as u32)
    }
}

#[derive(Debug, Default)]
struct Pendente {
    buf: Vec<u8>,
    largura: u32,
    altura: u32,
    ordem: Option<OrdemRgb>,
    novo: bool,
    chave: bool,
    bitrate: Option<u32>,
    parar: bool,
}

#[derive(Debug, Default)]
struct Compartilhado {
    pendente: Mutex<Pendente>,
    cv: Condvar,
}

impl Compartilhado {
    fn travar(&self) -> MutexGuard<'_, Pendente> {
        self.pendente.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Envio de vídeo de um monitor; parar = soltar.
#[derive(Debug)]
pub struct EnvioVideo {
    compartilhado: Arc<Compartilhado>,
    estatisticas: Arc<EstatisticasEnvio>,
    thread: Option<JoinHandle<()>>,
}

impl EnvioVideo {
    /// Começa o envio. O primeiro quadro sai como quadro-chave.
    pub fn iniciar(
        config: ConfigEnvio,
        encoder: Box<dyn VideoEncoder>,
        saida: Box<dyn SaidaDatagramas>,
    ) -> std::io::Result<Self> {
        let compartilhado = Arc::new(Compartilhado::default());
        let estatisticas = Arc::new(EstatisticasEnvio::default());
        let thread = std::thread::Builder::new()
            .name(format!("taildesk-envio-{}", config.stream_id))
            .spawn({
                let c = compartilhado.clone();
                let e = estatisticas.clone();
                move || laco(config, encoder, saida, &c, &e)
            })?;
        Ok(EnvioVideo {
            compartilhado,
            estatisticas,
            thread: Some(thread),
        })
    }

    /// Entrega um quadro capturado (copia e volta). Imagem inválida é ignorada.
    pub fn entregar(&self, q: &QuadroRgb<'_>) {
        if !q.valido() {
            return;
        }
        let linha = q.largura as usize * 4;
        let mut p = self.compartilhado.travar();
        if p.novo {
            self.estatisticas.pulados.fetch_add(1, Ordering::Relaxed);
        }
        p.buf.resize(linha * q.altura as usize, 0);
        for (y, dst) in p.buf.chunks_exact_mut(linha).enumerate() {
            dst.copy_from_slice(&q.dados[y * q.stride..y * q.stride + linha]);
        }
        p.largura = q.largura;
        p.altura = q.altura;
        p.ordem = Some(q.ordem);
        p.novo = true;
        drop(p);
        self.compartilhado.cv.notify_one();
    }

    /// O próximo quadro sai como quadro-chave (`RequestKeyframe`, `PictureLoss`).
    pub fn pedir_chave(&self) {
        self.compartilhado.travar().chave = true;
    }

    /// Novo alvo de taxa (seleção adaptativa); vale a partir do próximo quadro.
    pub fn ajustar_bitrate(&self, bps: u32) {
        self.compartilhado.travar().bitrate = Some(bps);
    }

    pub fn estatisticas(&self) -> &EstatisticasEnvio {
        &self.estatisticas
    }
}

impl Drop for EnvioVideo {
    fn drop(&mut self) {
        self.compartilhado.travar().parar = true;
        self.compartilhado.cv.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn laco(
    config: ConfigEnvio,
    mut encoder: Box<dyn VideoEncoder>,
    mut saida: Box<dyn SaidaDatagramas>,
    c: &Compartilhado,
    e: &EstatisticasEnvio,
) {
    let intervalo = Duration::from_secs_f32(1.0 / config.fps_max.max(1.0));
    let inicio = Instant::now();
    let mut proximo = inicio;
    let mut frag = Fragmentador::novo(config.stream_id);
    let mut imagem = Vec::new();
    let mut au = Vec::new();
    let mut chave = true;
    let mut bps = 0u32;
    loop {
        let (largura, altura, ordem) = {
            let mut p = c.travar();
            loop {
                if p.parar {
                    return;
                }
                let agora = Instant::now();
                if p.novo && agora >= proximo {
                    break;
                }
                p = if p.novo {
                    c.cv.wait_timeout(p, proximo - agora)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                } else {
                    c.cv.wait(p).unwrap_or_else(PoisonError::into_inner)
                };
            }
            std::mem::swap(&mut p.buf, &mut imagem);
            p.novo = false;
            chave |= std::mem::take(&mut p.chave);
            if let Some(b) = p.bitrate.take() {
                encoder.ajustar_bitrate(b);
                bps = b;
            }
            (p.largura, p.altura, p.ordem.unwrap_or(OrdemRgb::Bgrx))
        };
        let t = Instant::now();
        proximo = (proximo + intervalo).max(t);
        if !chave && saida.na_fila() > fila_maxima(bps) {
            e.pulados.fetch_add(1, Ordering::Relaxed);
            e.fila_cheia.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let q = QuadroRgb {
            largura,
            altura,
            stride: largura as usize * 4,
            ordem,
            dados: &imagem,
        };
        let tipo = match encoder.codificar(&q, chave, &mut au) {
            Ok(Some(tipo)) => tipo,
            Ok(None) => {
                e.pulados.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            Err(_) => {
                // O encoder pode ter avançado as referências sem o quadro sair: o próximo
                // tem de ser chave, ou o controlador decodifica em cima de um quadro que não
                // recebeu.
                e.erros.fetch_add(1, Ordering::Relaxed);
                chave = true;
                continue;
            }
        };
        e.encode_us
            .fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
        let Some(max) = saida.max_datagrama() else {
            e.erros.fetch_add(1, Ordering::Relaxed);
            chave = true;
            continue;
        };
        let e_chave = tipo == TipoQuadro::Chave;
        let pts = t.duration_since(inicio).as_micros() as u32;
        let mut n = 0u64;
        let ok = frag.fragmentar(&au, e_chave, pts, max, |d| {
            saida.enviar(d);
            n += 1;
        });
        if !ok {
            e.erros.fetch_add(1, Ordering::Relaxed);
            chave = true;
            continue;
        }
        chave = false;
        e.quadros.fetch_add(1, Ordering::Relaxed);
        e.tamanho.store(
            u64::from(largura) << 32 | u64::from(altura),
            Ordering::Relaxed,
        );
        e.chaves.fetch_add(u64::from(e_chave), Ordering::Relaxed);
        e.bytes.fetch_add(au.len() as u64, Ordering::Relaxed);
        e.datagramas.fetch_add(n, Ordering::Relaxed);
    }
}
