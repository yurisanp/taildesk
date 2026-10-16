//! Som do computador remoto (`15-audio.md`): o host captura o que toca e manda em Opus; o
//! controlador decodifica e toca.
//!
//! Desligar fecha tudo: soltar [`SomHost`] fecha a captura e para a thread do encoder antes do
//! `AudioStopped`, e soltar [`SomControlador`] fecha a saída de som.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use bytes::Bytes;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use taildesk_media::audio::{
    BITRATE_KBPS, CANAIS, Conversor, EnvioAudio, RecepcaoAudio, Reproducao, TAXA, fila,
};
use taildesk_platform::som::{self, FormatoSom, StreamSom};

/// Folga da fila entre o callback de captura e o encoder.
const FILA_CAPTURA_MS: u64 = 500;
/// Quanto o encoder espera entre duas leituras da fila (um quadro tem 20 ms).
const PASSO: Duration = Duration::from_millis(5);

/// Captura e envio do som de uma sessão no host.
#[derive(Debug)]
pub struct SomHost {
    // Ordem de drop: a captura fecha antes, depois a thread do encoder termina.
    captura: Option<StreamSom>,
    parar: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SomHost {
    /// Abre a captura e começa a mandar datagramas `kind=1` do `stream_id`. Bloqueia enquanto
    /// o dispositivo abre.
    pub fn iniciar(conn: quinn::Connection, stream_id: u8) -> Result<SomHost, String> {
        // A fila nasce antes de saber o formato: dimensiona pelo pior caso (192 kHz, 8 canais).
        let cap = 192_000 * 8 * FILA_CAPTURA_MS as usize / 1000;
        let (mut prod, mut cons) = HeapRb::<f32>::new(cap).split();
        let captura = som::capturar(move |a| {
            prod.push_slice(a);
        })?;
        let FormatoSom { taxa, canais } = captura.formato;
        eprintln!(
            "taildesk: som do sistema de {} ({taxa} Hz, {canais} canais)",
            captura.dispositivo
        );
        let mut envio = EnvioAudio::novo(stream_id, BITRATE_KBPS).map_err(|e| e.to_string())?;
        let parar = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("taildesk-som-envio".into())
            .spawn({
                let parar = parar.clone();
                move || {
                    let mut conversor = Conversor::novo(taxa, canais);
                    let mut bruto = vec![0.0f32; cap];
                    let mut pcm = Vec::with_capacity(cap);
                    while !parar.load(Ordering::Relaxed) {
                        std::thread::sleep(PASSO);
                        let n = cons.pop_slice(&mut bruto);
                        if n == 0 {
                            continue;
                        }
                        pcm.clear();
                        conversor.converter(&bruto[..n], &mut pcm);
                        let Some(max) = conn.max_datagram_size() else {
                            continue;
                        };
                        let r = envio.empurrar(&pcm, max, |d| {
                            // Fila cheia ou conexão fechada: o quadro se perde, o PLC cobre.
                            let _ = conn.send_datagram(Bytes::copy_from_slice(d));
                        });
                        if let Err(e) = r {
                            eprintln!("taildesk: opus: {e}");
                            return;
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(SomHost {
            captura: Some(captura),
            parar,
            thread: Some(thread),
        })
    }
}

impl Drop for SomHost {
    fn drop(&mut self) {
        self.captura = None;
        self.parar.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Onde a task dos datagramas entrega o áudio: `None` com o som desligado (o que chegar em voo
/// é descartado).
pub type Destino = Arc<Mutex<Option<(u8, RecepcaoAudio)>>>;

/// Reprodução do som no controlador; soltar fecha a saída e desliga o [`Destino`].
pub struct SomControlador {
    destino: Destino,
    _saida: StreamSom,
}

impl std::fmt::Debug for SomControlador {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SomControlador").finish_non_exhaustive()
    }
}

impl SomControlador {
    /// Abre a saída de som padrão para o `stream_id` do `AudioSelected`. Bloqueia enquanto o
    /// dispositivo abre.
    pub fn iniciar(stream_id: u8, destino: Destino) -> Result<SomControlador, String> {
        let (prod, cons) = fila(1000);
        let recepcao = RecepcaoAudio::nova(stream_id, prod).map_err(|e| e.to_string())?;
        let mut reproducao = Reproducao::nova(cons);
        // A saída abre em 48 kHz estéreo (o PipeWire e o WASAPI compartilhado convertem).
        let saida = som::tocar(move |buf, formato| {
            if formato.taxa == TAXA && usize::from(formato.canais) == CANAIS {
                reproducao.preencher(buf);
            } else {
                buf.fill(0.0);
            }
        })?;
        *travar(&destino) = Some((stream_id, recepcao));
        Ok(SomControlador {
            destino,
            _saida: saida,
        })
    }
}

impl Drop for SomControlador {
    fn drop(&mut self) {
        *travar(&self.destino) = None;
    }
}

fn travar(d: &Destino) -> std::sync::MutexGuard<'_, Option<(u8, RecepcaoAudio)>> {
    d.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Entrega um datagrama de áudio (`stream_id` no byte 1) à recepção, se o som está ligado.
pub fn receber(destino: &Destino, stream_id: u8, datagrama: &[u8]) {
    if let Some((s, r)) = travar(destino).as_mut()
        && *s == stream_id
    {
        r.receber(datagrama);
    }
}
