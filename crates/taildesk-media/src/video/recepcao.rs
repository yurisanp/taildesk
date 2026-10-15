//! Recepção de vídeo de um monitor no controlador: datagramas → quadro → decoder → planos.
//!
//! [`RecepcaoVideo::receber`] roda na task de rede: remonta o quadro e o passa para a thread de
//! decode por um canal limitado, com os buffers devolvidos para reuso. Fila cheia descarta o
//! quadro e pede um quadro-chave (H.264 não pula deltas). Os planos decodificados vão para o
//! callback na thread de decode, que repassa ao render (latest-wins do lado de lá).

use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use taildesk_codec::{QuadroYuv, VideoDecoder};

use super::fragmentos::{Remontador, Remontagem};

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

/// Recepção de vídeo de um monitor; parar = soltar.
pub struct RecepcaoVideo {
    remontador: Remontador,
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
        let thread = std::thread::Builder::new()
            .name(format!("taildesk-decode-{display_id}"))
            .spawn({
                let chave = chave.clone();
                move || {
                    for au in entrada {
                        let r = decoder.decodificar(&au, &mut |q| ao_quadro(display_id, q));
                        if r.is_err() {
                            chave.pedir();
                        }
                        let _ = devolver.try_send(au);
                    }
                }
            })?;
        Ok(RecepcaoVideo {
            remontador: Remontador::novo(stream_id),
            fila: Some(fila),
            devolvidos,
            chave,
            thread: Some(thread),
        })
    }

    /// Um datagrama de vídeo deste stream (os de outros streams são ignorados).
    pub fn receber(&mut self, datagrama: &[u8]) {
        match self.remontador.receber(datagrama) {
            Remontagem::Completo { .. } => {}
            Remontagem::PedirChave => return self.chave.pedir(),
            Remontagem::Parcial | Remontagem::Ignorado => return,
        }
        let Some(fila) = &self.fila else { return };
        let mut au = self.devolvidos.try_recv().unwrap_or_default();
        au.clear();
        au.extend_from_slice(self.remontador.quadro());
        match fila.try_send(au) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.remontador.esperar_chave();
                self.chave.pedir();
            }
            Err(TrySendError::Disconnected(_)) => self.fila = None,
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
