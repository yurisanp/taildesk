//! Recepção de vídeo de um monitor no controlador: datagramas → quadro → decoder → planos.
//!
//! [`RecepcaoVideo::receber`] roda na task de rede: remonta o quadro e o passa para a thread de
//! decode por um canal limitado, com os buffers devolvidos para reuso. Fila cheia descarta o
//! quadro e pede um quadro-chave (H.264 não pula deltas). Os planos decodificados vão para o
//! callback na thread de decode, que repassa ao render (latest-wins do lado de lá).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use taildesk_codec::{QuadroYuv, VideoDecoder};

use taildesk_proto::control::ReceiverReport;

use super::fragmentos::{Remontador, Remontagem};
use super::medidor::Medidor;

/// Quadros na fila do decode antes de descartar.
const FILA: usize = 4;
/// Intervalo mínimo entre dois pedidos de quadro-chave.
const INTERVALO_CHAVE: Duration = Duration::from_millis(250);

/// Chamado quando o controlador deve mandar `RequestKeyframe{display_id}` ao host.
pub type PedirChave = Arc<dyn Fn() + Send + Sync>;

/// Recebe os planos de cada quadro decodificado: `display_id` e o quadro.
pub type AoQuadro = Box<dyn FnMut(u32, &QuadroYuv<'_>) + Send>;

struct Limitador {
    pedir: PedirChave,
    ultimo: Mutex<Option<Instant>>,
}

impl Limitador {
    fn pedir(&self) {
        let mut u = self.ultimo.lock().unwrap_or_else(PoisonError::into_inner);
        if u.is_some_and(|t| t.elapsed() < INTERVALO_CHAVE) {
            return;
        }
        *u = Some(Instant::now());
        drop(u);
        (self.pedir)();
    }
}

/// Fila e tempo do decode, para o `ReceiverReport`.
#[derive(Debug, Default)]
struct Decode {
    na_fila: AtomicU32,
    quadros: AtomicU64,
    us: AtomicU64,
}

/// Recepção de vídeo de um monitor; parar = soltar.
pub struct RecepcaoVideo {
    remontador: Remontador,
    medidor: Medidor,
    decode: Arc<Decode>,
    /// `quadros` e `us` do decode no último relatório.
    decode_antes: (u64, u64),
    fila: Option<SyncSender<Vec<u8>>>,
    devolvidos: Receiver<Vec<u8>>,
    chave: Arc<Limitador>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for RecepcaoVideo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecepcaoVideo")
            .field("remontador", &self.remontador)
            .finish_non_exhaustive()
    }
}

impl RecepcaoVideo {
    /// Começa a recepção do stream `stream_id` (do `CodecSelected`) do monitor `display_id`.
    pub fn iniciar(
        display_id: u32,
        stream_id: u8,
        mut decoder: Box<dyn VideoDecoder>,
        mut ao_quadro: AoQuadro,
        pedir_chave: PedirChave,
    ) -> std::io::Result<Self> {
        let (fila, entrada) = sync_channel::<Vec<u8>>(FILA);
        let (devolver, devolvidos) = sync_channel::<Vec<u8>>(FILA + 2);
        let chave = Arc::new(Limitador {
            pedir: pedir_chave,
            ultimo: Mutex::new(None),
        });
        let decode = Arc::new(Decode::default());
        let thread = std::thread::Builder::new()
            .name(format!("taildesk-decode-{display_id}"))
            .spawn({
                let chave = chave.clone();
                let decode = decode.clone();
                move || {
                    for au in entrada {
                        let t = Instant::now();
                        let r = decoder.decodificar(&au, &mut |q| ao_quadro(display_id, q));
                        decode
                            .us
                            .fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
                        decode.quadros.fetch_add(1, Ordering::Relaxed);
                        decode.na_fila.fetch_sub(1, Ordering::Relaxed);
                        if r.is_err() {
                            chave.pedir();
                        }
                        let _ = devolver.try_send(au);
                    }
                }
            })?;
        Ok(RecepcaoVideo {
            remontador: Remontador::novo(stream_id),
            medidor: Medidor::novo(Instant::now()),
            decode,
            decode_antes: (0, 0),
            fila: Some(fila),
            devolvidos,
            chave,
            thread: Some(thread),
        })
    }

    /// Um datagrama de vídeo deste stream (os de outros streams são ignorados).
    pub fn receber(&mut self, datagrama: &[u8]) {
        self.medidor.datagrama(datagrama, Instant::now());
        match self.remontador.receber(datagrama) {
            Remontagem::Completo { .. } => {}
            Remontagem::PedirChave => return self.chave.pedir(),
            Remontagem::Parcial | Remontagem::Ignorado => return,
        }
        let Some(fila) = &self.fila else { return };
        let mut au = self.devolvidos.try_recv().unwrap_or_default();
        au.clear();
        au.extend_from_slice(self.remontador.quadro());
        self.decode.na_fila.fetch_add(1, Ordering::Relaxed);
        match fila.try_send(au) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.decode.na_fila.fetch_sub(1, Ordering::Relaxed);
                self.remontador.esperar_chave();
                self.chave.pedir();
            }
            Err(TrySendError::Disconnected(_)) => {
                self.decode.na_fila.fetch_sub(1, Ordering::Relaxed);
                self.fila = None;
            }
        }
    }

    /// `ReceiverReport` do intervalo desde o anterior (o controlador manda a cada 250 ms).
    pub fn relatorio(&mut self, display_id: u32, agora: Instant) -> ReceiverReport {
        let quadros = self.decode.quadros.load(Ordering::Relaxed);
        let us = self.decode.us.load(Ordering::Relaxed);
        let (q0, us0) = std::mem::replace(&mut self.decode_antes, (quadros, us));
        ReceiverReport {
            decode_us: (us - us0)
                .checked_div(quadros - q0)
                .unwrap_or(0)
                .try_into()
                .unwrap_or(u32::MAX),
            queue_frames: self.decode.na_fila.load(Ordering::Relaxed),
            ..self.medidor.relatorio(display_id, agora)
        }
    }
}

impl Drop for RecepcaoVideo {
    fn drop(&mut self) {
        self.fila = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
