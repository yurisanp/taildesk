//! Sessão de rede do celular com um host: o `Sessao` do `taildesk-session` (controle, consentimento,
//! eventos) mais a recepção de vídeo do monitor que está na tela, um por vez (MF-47).
//!
//! Os datagramas são lidos numa task e entregues ao `RecepcaoVideo` do stream; o decode roda na
//! thread dele e guarda o quadro na [`Caixa`]; `acordar` avisa a interface (latest-wins).

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use taildesk_codec::{ErroCodec, QuadroYuv, VideoDecoder};
use taildesk_media::video::RecepcaoVideo;
use taildesk_proto::control::{CodecSelected, Control, Hello, Msg, RequestKeyframe};
use taildesk_session::conexao::controlador::{EventoControlador, Sessao};
use taildesk_transport::Pin;
use tokio::sync::mpsc;

use crate::video::Caixa;

/// Abre o decoder H.264 para um stream (`largura`×`altura` do `CodecSelected`).
pub type AbrirDecoder =
    Arc<dyn Fn(u32, u32) -> Result<Box<dyn VideoDecoder>, ErroCodec> + Send + Sync>;

/// O decoder deste aparelho: MediaCodec no Android; no Linux (desenvolvimento) o openh264, se a
/// feature estiver ligada.
pub fn decoder_do_sistema() -> AbrirDecoder {
    Arc::new(|largura, altura| {
        #[cfg(target_os = "android")]
        {
            Ok(
                Box::new(taildesk_codec::mediacodec::DecoderMediaCodec::novo(
                    largura, altura,
                )?) as Box<dyn VideoDecoder>,
            )
        }
        #[cfg(all(not(target_os = "android"), feature = "openh264"))]
        {
            let _ = (largura, altura);
            Ok(Box::new(taildesk_codec::DecoderOpenH264::novo()?) as Box<dyn VideoDecoder>)
        }
        #[cfg(all(not(target_os = "android"), not(feature = "openh264")))]
        {
            let _ = (largura, altura);
            Err(ErroCodec::Backend("sem decoder H.264 neste build".into()))
        }
    })
}

type Recepcoes = Arc<Mutex<HashMap<u8, (u32, RecepcaoVideo)>>>;

fn travar(r: &Recepcoes) -> MutexGuard<'_, HashMap<u8, (u32, RecepcaoVideo)>> {
    r.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Uma sessão aberta.
pub struct SessaoRede {
    sessao: Sessao,
    recepcoes: Recepcoes,
    som: crate::som::Destino,
    pub caixa: Arc<Caixa>,
    /// Bytes recebidos em datagramas (consumo da sessão, MF-81).
    pub bytes: Arc<AtomicU64>,
    datagramas: tokio::task::JoinHandle<()>,
    acordar: Arc<dyn Fn() + Send + Sync>,
    abrir: AbrirDecoder,
}

impl std::fmt::Debug for SessaoRede {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessaoRede")
            .field("sessao", &self.sessao)
            .finish_non_exhaustive()
    }
}

impl SessaoRede {
    /// Conecta e manda o `Hello`. `acordar` é chamado (de outra thread) quando há quadro novo.
    pub async fn conectar(
        endpoint: &quinn::Endpoint,
        remoto: SocketAddr,
        pin: Pin,
        hello: Hello,
        nome: &str,
        abrir: AbrirDecoder,
        acordar: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<(SessaoRede, mpsc::Receiver<EventoControlador>)> {
        let (sessao, eventos) = Sessao::conectar(endpoint, remoto, pin, hello, nome).await?;
        let recepcoes: Recepcoes = Arc::default();
        let som = crate::som::Destino::default();
        let bytes = Arc::new(AtomicU64::new(0));
        let datagramas = tokio::spawn({
            let conn = sessao.conexao().clone();
            let recepcoes = recepcoes.clone();
            let som = som.clone();
            let bytes = bytes.clone();
            async move {
                while let Ok(d) = conn.read_datagram().await {
                    bytes.fetch_add(d.len() as u64, Ordering::Relaxed);
                    let Some(&stream) = d.get(1) else { continue };
                    if stream >= taildesk_proto::media::AUDIO_STREAM_MIN {
                        crate::som::receber(&som, stream, &d);
                        continue;
                    }
                    if let Some((_, r)) = travar(&recepcoes).get_mut(&stream) {
                        r.receber(&d);
                    }
                }
            }
        });
        Ok((
            SessaoRede {
                sessao,
                recepcoes,
                som,
                caixa: Arc::default(),
                bytes,
                datagramas,
                acordar,
                abrir,
            },
            eventos,
        ))
    }

    /// Fingerprint do certificado do host (para fixar no primeiro contato).
    pub fn fingerprint(&self) -> Option<[u8; 32]> {
        self.sessao.fingerprint()
    }

    /// Onde os datagramas de áudio vão (o som abre e fecha por [`crate::som::Saida`]).
    pub fn som(&self) -> crate::som::Destino {
        self.som.clone()
    }

    /// Serviço de arquivos da sessão (barato de clonar).
    pub fn arquivos(&self) -> taildesk_files::service::Files<quinn::Connection> {
        self.sessao.arquivos().clone()
    }

    /// Manda sem esperar (da thread da interface); fila cheia descarta.
    pub fn enviar(&self, msg: Msg) {
        let _ = self.sessao.saida().try_send(Control { msg: Some(msg) });
    }

    pub fn rtt_ms(&self) -> u32 {
        u32::try_from(self.sessao.rtt().as_millis()).unwrap_or(u32::MAX)
    }

    /// O host escolheu o codec de um monitor: começa a receber esse stream (e larga os outros:
    /// só um monitor na tela por vez).
    pub fn codec_escolhido(&self, c: &CodecSelected) -> Result<(), ErroCodec> {
        let decoder = (self.abrir)(c.width, c.height)?;
        let caixa = self.caixa.clone();
        let acordar = self.acordar.clone();
        let mut rascunho = Vec::new();
        let ao_quadro = Box::new(move |_: u32, q: &QuadroYuv<'_>| {
            if caixa.guardar(q, &mut rascunho) {
                acordar();
            }
        });
        let saida = self.sessao.saida();
        let display = c.display_id;
        let pedir_chave = Arc::new(move || {
            let _ = saida.try_send(Control {
                msg: Some(Msg::RequestKeyframe(RequestKeyframe {
                    display_id: display,
                })),
            });
        });
        let stream = u8::try_from(c.stream_id).map_err(|_| ErroCodec::QuadroInvalido)?;
        let r = RecepcaoVideo::iniciar(display, stream, decoder, ao_quadro, pedir_chave)
            .map_err(|e| ErroCodec::Backend(e.to_string()))?;
        let mut m = travar(&self.recepcoes);
        m.clear();
        m.insert(stream, (display, r));
        Ok(())
    }

    /// O host parou o vídeo de um monitor.
    pub fn video_parou(&self, display_id: u32) {
        travar(&self.recepcoes).retain(|_, (d, _)| *d != display_id);
    }

    /// `ReceiverReport` de cada stream recebido (mande a cada 250 ms).
    pub fn relatorios(&self) -> Vec<Msg> {
        let agora = Instant::now();
        travar(&self.recepcoes)
            .values_mut()
            .map(|(d, r)| Msg::ReceiverReport(r.relatorio(*d, agora)))
            .collect()
    }

    /// Encerra pelo celular.
    pub async fn encerrar(self) {
        self.datagramas.abort();
        travar(&self.recepcoes).clear();
        *self
            .som
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.sessao.encerrar().await;
    }
}
