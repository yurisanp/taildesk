//! Vídeo do host (`13-multimonitor.md` §3): a captura de cada monitor pedido (DXGI no Windows,
//! PipeWire pela sessão do portal no Wayland) vai para um [`EnvioVideo`] (encoder → datagramas),
//! e o cursor sai à parte: posição pelo stream de controle e forma pelo stream de cursor
//! (`taildesk_proto::cursor`), para o controlador desenhar por cima do vídeo.
//!
//! A captura abre no primeiro `StartVideo` e cobre todos os monitores; os que ninguém pediu
//! ficam pausados (`ativar(id, false)`). Soltar a [`MidiaApp`] (fim da sessão) para tudo.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use taildesk_codec::{CodecVideo, ConfigEncoder, Opcao, OrdemRgb, QuadroRgb};
use taildesk_media::escada::{Escada, Modo, SinaisEscada};
use taildesk_media::taxa::{ControleTaxa, Relato, Sinais};
use taildesk_media::video::{ConfigEnvio, EnvioVideo};
use taildesk_platform::captura::{FormaCursor, FormatoPixel, Quadro};
use taildesk_proto::control::{
    AudioCodec, AudioSelected, AudioStopReason, AudioStopped, Codec, CodecCap, CodecSelected,
    ColorInfo, CursorPos, DisplayInfo, DisplaysChanged, Error, ErrorCode, Msg, Priority,
    QualityMode, QualityPrefs, SetVideoQuality, StartAudio, StartVideo, VideoStats,
    VideoStopReason, VideoStopped,
};
use taildesk_proto::cursor::{STREAM_MAGIC, ShapeHeader};
use taildesk_proto::media::AUDIO_STREAM_MIN;
use taildesk_session::conexao::host::{CanalMidia, Midia};
use taildesk_session::{AcaoForma, CursorHost};
use taildesk_transport::SaidaQuic;

use crate::som::SomHost;
use tokio::sync::mpsc;

/// Streams de vídeo ao mesmo tempo (`ErrorCode::TooManyStreams`): até 4 com encoder de hardware,
/// mas só 2 em software (openh264), que já pesa em 1440p (`13-multimonitor.md` §5).
const MAX_FLUXOS: usize = 4;
const MAX_FLUXOS_SOFTWARE: usize = 2;
/// openh264 em software: 30 fps com folga em 1080p.
const FPS_SOFTWARE: f32 = 30.0;
/// NVENC e VA-API codificam 1080p em menos de 10 ms.
const FPS_HARDWARE: f32 = 60.0;
/// Intervalo do `VideoStats` para o painel de diagnóstico do controlador.
const INTERVALO_STATS: Duration = Duration::from_secs(1);
/// Laço rápido do controle de taxa (`taildesk_media::taxa`).
const INTERVALO_TAXA: Duration = Duration::from_millis(100);
/// Subidas de taxa espaçadas: o openh264 recria o encoder (e manda um quadro-chave) a cada
/// mudança. Cortes valem na hora.
const INTERVALO_SUBIDA: Duration = Duration::from_secs(2);
/// Alvo inicial, antes do primeiro quadro dizer o tamanho real da captura.
const BITRATE_BPS: u32 = 8_000_000;

/// Codecs que o host pode oferecer, do melhor para o pior (o `HelloAck.encode_caps`).
const CODECS: [CodecVideo; 4] = [
    CodecVideo::Av1,
    CodecVideo::Hevc,
    CodecVideo::H264,
    CodecVideo::PyroWave,
];

/// Codec do protocolo que o TailDesk sabe codificar.
fn codec_video(c: i32) -> Option<CodecVideo> {
    match Codec::try_from(c).ok()? {
        Codec::H264 => Some(CodecVideo::H264),
        Codec::Hevc => Some(CodecVideo::Hevc),
        Codec::Av1 => Some(CodecVideo::Av1),
        Codec::PyroWave => Some(CodecVideo::PyroWave),
        _ => None,
    }
}

/// Codecs do `Hello.decode_caps` que o TailDesk sabe codificar (vazio = controlador antigo, só
/// H.264).
fn codecs_aceitos(caps: &[CodecCap]) -> Vec<CodecVideo> {
    caps.iter().filter_map(|c| codec_video(c.codec)).collect()
}

/// Codecs com encoder nesta máquina, para o `HelloAck` (a lista sai uma vez por execução: listar
/// as GPUs custa alguns milissegundos). [`preparar_encode_caps`] adianta isso no início.
pub fn encode_caps() -> Vec<CodecCap> {
    static CAPS: OnceLock<Vec<CodecCap>> = OnceLock::new();
    CAPS.get_or_init(|| {
        CODECS
            .into_iter()
            .filter_map(|c| {
                let opcoes = taildesk_codec::opcoes_encoder(c);
                let primeira = opcoes.first()?;
                Some(CodecCap {
                    codec: codec_proto(c).into(),
                    hw: primeira.hardware(),
                    ..CodecCap::default()
                })
            })
            .collect()
    })
    .clone()
}

/// Calcula [`encode_caps`] fora da thread da rede, antes da primeira sessão.
pub async fn preparar_encode_caps() {
    let _ = tokio::task::spawn_blocking(encode_caps).await;
}

/// Encoders a tentar, em ordem: os do codec que o usuário escolheu (se o controlador decodifica)
/// e depois os do automático, como reserva se nenhum do escolhido abrir.
fn candidatos(aceitos: &[CodecVideo], pedido: Option<CodecVideo>) -> Vec<Opcao> {
    let mut lista = match pedido {
        Some(c) if aceitos.contains(&c) || c == CodecVideo::H264 => {
            taildesk_codec::opcoes_encoder(c)
        }
        _ => Vec::new(),
    };
    for o in taildesk_codec::candidatos_encoder(aceitos) {
        if !lista.contains(&o) {
            lista.push(o);
        }
    }
    lista
}

/// Escada do laço lento pela preferência do controlador (`QualityPrefs.mode`).
fn modo(p: &QualityPrefs) -> Modo {
    match QualityMode::try_from(p.mode).unwrap_or(QualityMode::Auto) {
        QualityMode::Text => Modo::Nitidez,
        QualityMode::Motion => Modo::Fluidez,
        QualityMode::LowBandwidth => Modo::Economia,
        QualityMode::Auto => Modo::Auto,
    }
}

/// Teto de FPS: o do encoder, ou o do controlador (`QualityPrefs.fps_max`) se for menor.
fn fps_pedido(p: &QualityPrefs, encoder: f32) -> f32 {
    match p.fps_max {
        0 => encoder,
        f => (f as f32).min(encoder),
    }
}

/// Fator do laço rápido a partir do qual a rede conta como folgada para o laço lento subir.
const FATOR_LIVRE: f64 = 0.98;
/// Fila no caminho abaixo disso conta como folga.
const FILA_LIVRE: Duration = Duration::from_millis(5);
/// Monitor `Visible` (janela sem foco) com outro `Focused` saindo: teto de FPS e parte da taxa
/// (`13-multimonitor.md` §2.3). O focado fica com o resto da banda.
const FPS_VISIVEL: f32 = 15.0;
const PARTE_VISIVEL: f64 = 0.3;

/// Taxa pedida pelo controlador (`QualityPrefs.bitrate_kbps`), em bit/s; `None` = automática.
fn taxa_pedida(p: &QualityPrefs) -> Option<u32> {
    (p.bitrate_kbps > 0).then(|| p.bitrate_kbps.saturating_mul(1000))
}

fn codec_proto(c: CodecVideo) -> Codec {
    match c {
        CodecVideo::H264 => Codec::H264,
        CodecVideo::Hevc => Codec::Hevc,
        CodecVideo::Av1 => Codec::Av1,
        CodecVideo::PyroWave => Codec::PyroWave,
    }
}

/// Alvo de taxa para o tamanho e o fps: 0,07 bit por pixel por quadro, entre 4 e 30 Mbit/s
/// (1080p60 ≈ 8,7 Mbit/s; 2560x1600 a 60 fps ≈ 17 Mbit/s). A seleção adaptativa refina depois.
fn taxa_para(largura: u32, altura: u32, fps: f32) -> u32 {
    let bps = f64::from(largura) * f64::from(altura) * f64::from(fps) * 0.07;
    bps.clamp(4e6, 30e6) as u32
}
/// Eventos de cursor esperando a task (posição descartável; forma é rara).
const FILA_CURSOR: usize = 64;

/// Cursor capturado, já normalizado (0..=65535 no monitor) e com a forma em RGBA.
#[derive(Debug)]
struct CursorCapturado {
    display_id: u32,
    visivel: bool,
    x: u16,
    y: u16,
    forma: Option<(ShapeHeader, Vec<u8>)>,
}

/// Contadores do envio no último `VideoStats`.
#[derive(Debug, Default, Clone, Copy)]
struct Amostra {
    quadros: u64,
    bytes: u64,
    encode_us: u64,
    pulados: u64,
    chaves: u64,
}

/// Vídeo de um monitor saindo.
#[derive(Debug)]
struct Fluxo {
    envio: EnvioVideo,
    encoder: &'static str,
    gpu: String,
    codec: CodecVideo,
    /// Alvo atual do encoder: o teto vezes o fator do controle de taxa.
    bitrate_bps: u32,
    /// Teto: a taxa do tamanho da imagem, ou a que o usuário fixou (cortada se `visivel`).
    teto_bps: u32,
    /// Taxa escolhida pelo usuário: o ajuste pelo tamanho da imagem não mexe no teto.
    taxa_usuario: Option<u32>,
    /// FPS com que o encoder abriu.
    fps: f32,
    /// Teto de FPS pedido pelo controlador (no máximo o do encoder).
    fps_usuario: f32,
    modo: Modo,
    /// Laço lento: resolução e FPS que saem agora.
    escada: Escada,
    /// Prioridade pedida pelo controlador (`StartVideo`, `SetPriority`).
    prioridade: Priority,
    /// Corte de `Visible` aplicado agora (só com outro monitor `Focused` saindo).
    visivel: bool,
    anterior: (Instant, Amostra),
    ultima_subida: Option<Instant>,
}

impl Fluxo {
    fn amostra(&self) -> Amostra {
        let e = self.envio.estatisticas();
        Amostra {
            quadros: e.quadros.load(Ordering::Relaxed),
            bytes: e.bytes.load(Ordering::Relaxed),
            encode_us: e.encode_us.load(Ordering::Relaxed),
            pulados: e.pulados.load(Ordering::Relaxed),
            chaves: e.chaves.load(Ordering::Relaxed),
        }
    }

    /// Teto automático pelo tamanho físico (que só se sabe pela captura; no Wayland com escala,
    /// maior que o lógico) no degrau atual do laço lento. `None` antes do primeiro quadro.
    fn teto_automatico(&self) -> Option<u32> {
        let captura = self.envio.estatisticas().captura();
        (captura.0 > 0).then(|| {
            let f = self.escada.formato(captura);
            taxa_para(f.largura, f.altura, f.fps)
        })
    }

    /// Teto pela taxa do usuário ou pelo tamanho, com o corte de `Visible`.
    fn teto_desejado(&self) -> Option<u32> {
        let base = self.taxa_usuario.or_else(|| self.teto_automatico())?;
        Some(if self.visivel {
            ((f64::from(base) * PARTE_VISIVEL) as u32).max(ControleTaxa::PISO_BPS as u32)
        } else {
            base
        })
    }

    fn atualizar_teto(&mut self) {
        if let Some(t) = self.teto_desejado() {
            self.teto_bps = t;
        }
    }

    /// Teto de FPS para a escada: o do usuário, e no máximo [`FPS_VISIVEL`] se `visivel`.
    fn fps_teto(&self) -> f32 {
        if self.visivel {
            self.fps_usuario.min(FPS_VISIVEL)
        } else {
            self.fps_usuario
        }
    }

    /// Passa preferência e teto de FPS à escada (o degrau da rede fica).
    fn preferir(&mut self, agora: Instant) {
        if self.escada.preferir(self.modo, self.fps_teto(), agora) {
            self.aplicar_degrau();
        }
    }

    /// Liga ou desliga o corte de `Visible`. Vale na hora: FPS no próximo quadro e a taxa já
    /// ajustada (ao ganhar o foco, o teto inteiro; o laço rápido corta se não couber).
    fn tornar_visivel(&mut self, visivel: bool, agora: Instant) {
        if visivel == self.visivel {
            return;
        }
        self.visivel = visivel;
        self.preferir(agora);
        self.atualizar_teto();
        let alvo = if visivel {
            self.bitrate_bps.min(self.teto_bps)
        } else {
            self.teto_bps
        };
        self.envio.ajustar_bitrate(alvo);
        self.bitrate_bps = alvo;
    }

    /// Passa o degrau atual da escada ao envio.
    fn aplicar_degrau(&self) {
        let d = self.escada.degrau();
        self.envio.ajustar_formato(d.altura_max, d.fps);
    }

    /// Um passo do laço lento. `true` se o degrau mudou (o teto já foi recalculado).
    fn passo_escada(&mut self, display_id: u32, agora: Instant, livre: bool) -> bool {
        let captura = self.envio.estatisticas().captura();
        let Some((f, motivo)) = self.escada.passo(&SinaisEscada {
            agora,
            alvo_bps: u64::from(self.bitrate_bps),
            teto_bps: u64::from(self.teto_bps),
            livre,
            captura,
        }) else {
            return false;
        };
        eprintln!(
            "taildesk: vídeo do monitor {display_id} em {}x{} a {} fps ({})",
            f.largura,
            f.altura,
            f.fps,
            motivo.texto()
        );
        self.aplicar_degrau();
        self.atualizar_teto();
        true
    }

    /// Alvo novo pelo fator do controle de taxa. Mudança menor que 10 % não vale um ajuste.
    fn aplicar(&mut self, fator: f64, agora: Instant) {
        let piso = ControleTaxa::PISO_BPS as u32;
        let alvo = ((f64::from(self.teto_bps) * fator) as u32)
            .max(piso.min(self.teto_bps))
            .min(self.teto_bps);
        let atual = self.bitrate_bps;
        let longe = alvo.abs_diff(atual) > atual / 10 || (alvo == self.teto_bps && alvo != atual);
        if !longe {
            return;
        }
        if alvo > atual {
            if self
                .ultima_subida
                .is_some_and(|t| agora.duration_since(t) < INTERVALO_SUBIDA)
            {
                return;
            }
            self.ultima_subida = Some(agora);
        }
        self.envio.ajustar_bitrate(alvo);
        self.bitrate_bps = alvo;
    }

    /// Estatísticas desde a última chamada.
    fn stats(&mut self, display_id: u32) -> VideoStats {
        let (agora, a) = (Instant::now(), self.amostra());
        let (antes, b) = std::mem::replace(&mut self.anterior, (agora, a));
        let segundos = agora.duration_since(antes).as_secs_f64().max(1e-3);
        let quadros = a.quadros - b.quadros;
        let (largura, altura) = self.envio.estatisticas().tamanho();
        VideoStats {
            display_id,
            encoder: self.encoder.into(),
            gpu: self.gpu.clone(),
            fps_x100: (quadros as f64 * 100.0 / segundos) as u32,
            bitrate_kbps: ((a.bytes - b.bytes) as f64 * 8.0 / 1000.0 / segundos) as u32,
            target_kbps: self.bitrate_bps / 1000,
            encode_us: (a.encode_us - b.encode_us)
                .checked_div(quadros)
                .unwrap_or(0)
                .try_into()
                .unwrap_or(u32::MAX),
            skipped: (a.pulados - b.pulados).try_into().unwrap_or(u32::MAX),
            keyframes: (a.chaves - b.chaves).try_into().unwrap_or(u32::MAX),
            loss_ppm: 0,
            width: largura,
            height: altura,
            est_kbps: 0,
            fps_max: self.escada.degrau().fps.round() as u32,
            reduced: self.escada.reduzido(self.envio.estatisticas().captura()),
        }
    }
}

/// O que a thread da captura enxerga.
#[derive(Debug)]
struct Compartilhado {
    /// Vídeo de cada monitor pedido (`display_id` → fluxo).
    fluxos: Mutex<HashMap<u32, Fluxo>>,
    cursor: mpsc::Sender<CursorCapturado>,
    /// Último `ReceiverReport` do controlador e quando chegou.
    relato: Mutex<Option<(Instant, Relato)>>,
}

impl Compartilhado {
    fn fluxos(&self) -> MutexGuard<'_, HashMap<u32, Fluxo>> {
        self.fluxos.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn ordem(f: FormatoPixel) -> OrdemRgb {
    match f {
        FormatoPixel::Bgrx | FormatoPixel::Bgra => OrdemRgb::Bgrx,
        FormatoPixel::Rgbx | FormatoPixel::Rgba => OrdemRgb::Rgbx,
    }
}

fn normalizar(v: i32, tamanho: u32) -> u16 {
    let max = tamanho.saturating_sub(1).max(1);
    (i64::from(v.clamp(0, max as i32)) * 65535 / i64::from(max)) as u16
}

/// Forma capturada → RGBA com o hash do bitmap. `None` se passar dos limites do protocolo.
fn forma_rgba(f: &FormaCursor<'_>) -> Option<(ShapeHeader, Vec<u8>)> {
    let (w, h) = (
        u16::try_from(f.largura).ok()?,
        u16::try_from(f.altura).ok()?,
    );
    let mut header = ShapeHeader {
        hash: 0,
        width: w,
        height: h,
        hot_x: f.hotspot_x.clamp(0, i32::from(w) - 1) as u16,
        hot_y: f.hotspot_y.clamp(0, i32::from(h) - 1) as u16,
    };
    if !header.is_valid() || f.stride < f.largura as usize * 4 {
        return None;
    }
    let mut pixels = Vec::with_capacity(header.pixels_len());
    for y in 0..f.altura as usize {
        let linha = f
            .dados
            .get(y * f.stride..y * f.stride + f.largura as usize * 4)?;
        for p in linha.chunks_exact(4) {
            pixels.extend_from_slice(&match f.formato {
                FormatoPixel::Bgra => [p[2], p[1], p[0], p[3]],
                FormatoPixel::Bgrx => [p[2], p[1], p[0], 255],
                FormatoPixel::Rgba => [p[0], p[1], p[2], p[3]],
                FormatoPixel::Rgbx => [p[0], p[1], p[2], 255],
            });
        }
    }
    // FNV-1a do bitmap e da geometria: a mesma forma dá o mesmo hash em toda sessão.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in header.encode()[8..].iter().chain(&pixels) {
        hash = (hash ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    header.hash = hash;
    Some((header, pixels))
}

/// Callback da captura: entrega a imagem ao envio do monitor e o cursor à task do cursor.
/// Uma cópia por thread de captura (DXGI tem uma por monitor).
#[derive(Clone)]
struct AoQuadro {
    compartilhado: Arc<Compartilhado>,
    /// Tamanho de cada monitor, para normalizar o cursor (o da última imagem vale mais).
    tamanhos: HashMap<u32, (u32, u32)>,
}

impl AoQuadro {
    fn quadro(&mut self, q: &Quadro<'_>) {
        if let Some(img) = &q.imagem {
            self.tamanhos
                .insert(q.display_id, (img.largura, img.altura));
            if let Some(f) = self.compartilhado.fluxos().get(&q.display_id) {
                f.envio.entregar(&QuadroRgb {
                    largura: img.largura,
                    altura: img.altura,
                    stride: img.stride,
                    ordem: ordem(img.formato),
                    dados: img.dados,
                });
            }
        }
        let Some(c) = &q.cursor else { return };
        // Só o cursor dos monitores que alguém está vendo.
        if !self.compartilhado.fluxos().contains_key(&q.display_id) {
            return;
        }
        let (w, h) = self.tamanhos.get(&q.display_id).copied().unwrap_or((1, 1));
        let _ = self.compartilhado.cursor.try_send(CursorCapturado {
            display_id: q.display_id,
            visivel: c.visivel,
            x: normalizar(c.x, w),
            y: normalizar(c.y, h),
            forma: c.forma.as_ref().and_then(forma_rgba),
        });
    }
}

/// Manda posição e forma do cursor; abre o stream de cursor na primeira forma.
async fn cursor(canal: CanalMidia, mut rx: mpsc::Receiver<CursorCapturado>) {
    let mut host = CursorHost::new();
    let mut stream: Option<quinn::SendStream> = None;
    while let Some(c) = rx.recv().await {
        if let Some((header, pixels)) = c.forma {
            let r = match host.forma(header.hash) {
                AcaoForma::Nada => None,
                AcaoForma::Ref(r) => Some(r),
                AcaoForma::EnviarERef(r) => {
                    if stream.is_none() {
                        stream = match canal.conn.open_uni().await {
                            Ok(mut s) => s.write_all(&STREAM_MAGIC).await.is_ok().then_some(s),
                            Err(_) => None,
                        };
                    }
                    let Some(s) = &mut stream else { return };
                    if s.write_all(&header.encode()).await.is_err()
                        || s.write_all(&pixels).await.is_err()
                    {
                        return;
                    }
                    Some(r)
                }
            };
            if let Some(r) = r
                && !canal.enviar(Msg::CursorShapeRef(r)).await
            {
                return;
            }
        }
        let pos = CursorPos {
            display_id: c.display_id,
            x: c.x.into(),
            y: c.y.into(),
            visible: c.visivel,
        };
        if let Some(p) = host.posicao(pos) {
            canal.tentar_enviar(Msg::CursorPos(p));
        }
    }
}

/// Manda o `VideoStats` de cada monitor saindo, uma vez por [`INTERVALO_STATS`].
async fn stats(canal: CanalMidia, compartilhado: Arc<Compartilhado>) {
    let mut t = tokio::time::interval(INTERVALO_STATS);
    t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let caminho = |c: &quinn::Connection| {
        let p = c.stats().path;
        (p.sent_packets, p.lost_packets)
    };
    let mut antes = caminho(&canal.conn);
    let mut perda = Perda::default();
    loop {
        t.tick().await;
        let agora = caminho(&canal.conn);
        let loss_ppm = perda.medir(agora.0 - antes.0, agora.1 - antes.1);
        antes = agora;
        let est_kbps = (banda_bps(&canal.conn) / 1000).min(u64::from(u32::MAX)) as u32;
        let msgs: Vec<VideoStats> = compartilhado
            .fluxos()
            .iter_mut()
            .map(|(&id, f)| VideoStats {
                loss_ppm,
                est_kbps,
                ..f.stats(id)
            })
            .collect();
        for m in msgs {
            // Informativo: fila cheia perde esta amostra e a próxima vem em um segundo.
            canal.tentar_enviar(Msg::VideoStats(m));
        }
    }
}

/// Perda numa janela dos últimos segundos. Com a tela parada saem só alguns pacotes por segundo
/// (cursor, estatísticas, keep-alive) e um único perdido viraria "perda de 30 %": abaixo de
/// [`Perda::MIN_PACOTES`] na janela a amostra é pequena demais e a perda conta como zero.
#[derive(Debug, Default)]
struct Perda {
    janela: std::collections::VecDeque<(u64, u64)>,
}

impl Perda {
    const JANELA: usize = 5;
    const MIN_PACOTES: u64 = 200;

    /// Pacotes enviados e perdidos no último intervalo → perda da janela em ppm.
    fn medir(&mut self, enviados: u64, perdidos: u64) -> u32 {
        if self.janela.len() == Self::JANELA {
            self.janela.pop_front();
        }
        self.janela.push_back((enviados, perdidos));
        let (e, p) = self
            .janela
            .iter()
            .fold((0u64, 0u64), |(e, p), (a, b)| (e + a, p + b));
        if e < Self::MIN_PACOTES {
            return 0;
        }
        (p.saturating_mul(1_000_000) / e).min(1_000_000) as u32
    }
}

/// Banda que a janela de congestionamento permite (bytes por RTT), em kbit/s.
fn estimar_kbps(cwnd: u64, rtt: Duration) -> u32 {
    let us = rtt.as_micros().max(100) as u64;
    (cwnd.saturating_mul(8_000) / us).min(u64::from(u32::MAX)) as u32
}

/// Banda do caminho em bit/s: a taxa de pacing do BBR, ou a janela ÷ RTT sem ela.
fn banda_bps(conn: &quinn::Connection) -> u64 {
    let m = conn.congestion_state().metrics();
    m.pacing_rate
        .unwrap_or_else(|| u64::from(estimar_kbps(m.congestion_window, conn.rtt())) * 1000)
}

/// Laço rápido do controle de taxa: a cada [`INTERVALO_TAXA`], o fator de
/// [`ControleTaxa`] vezes o teto de cada monitor vira o alvo do encoder.
async fn controle_taxa(canal: CanalMidia, compartilhado: Arc<Compartilhado>) {
    let mut t = tokio::time::interval(INTERVALO_TAXA);
    t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut controle = ControleTaxa::default();
    let mut motivo = controle.motivo();
    loop {
        t.tick().await;
        let agora = Instant::now();
        let relato = *compartilhado
            .relato
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (teto_bps, monitores) = {
            let mut f = compartilhado.fluxos();
            // `Visible` só perde banda para um `Focused` que esteja saindo.
            let algum_focado = f.len() > 1 && f.values().any(|x| x.prioridade != Priority::Visible);
            for x in f.values_mut() {
                x.tornar_visivel(algum_focado && x.prioridade == Priority::Visible, agora);
                x.atualizar_teto();
            }
            (
                f.values().map(|f| u64::from(f.teto_bps)).sum::<u64>(),
                f.len() as u32,
            )
        };
        let rtt = canal.conn.rtt();
        let mut fator = controle.passo(&Sinais {
            agora,
            rtt,
            banda_bps: Some(banda_bps(&canal.conn)),
            relato,
            teto_bps,
            monitores,
        });
        // Laço lento: com a taxa apertada, menos pixels ou menos quadros. Ao descer, o fator
        // sobe na mesma proporção que o teto caiu, para a taxa não despencar junto; ao subir,
        // fica onde está e o laço rápido corta se a rede não aguentar.
        let livre = fator >= FATOR_LIVRE && controle.fila(rtt) < FILA_LIVRE;
        let depois = {
            let mut f = compartilhado.fluxos();
            let mut mudou = false;
            for (&id, fluxo) in f.iter_mut() {
                mudou |= fluxo.passo_escada(id, agora, livre);
            }
            mudou.then(|| f.values().map(|f| u64::from(f.teto_bps)).sum::<u64>())
        };
        if let Some(depois) = depois
            && depois < teto_bps
        {
            controle.reescalar(teto_bps, depois);
            fator = controle.fator();
        }
        if controle.motivo() != motivo {
            motivo = controle.motivo();
            eprintln!(
                "taildesk: taxa do vídeo em {:.0} % do teto ({})",
                fator * 100.0,
                motivo.texto()
            );
        }
        for f in compartilhado.fluxos().values_mut() {
            f.aplicar(fator, agora);
        }
    }
}

/// O que mudou nesta máquina durante a sessão ([`Midia::evento`], `13-multimonitor.md` §4.3).
#[derive(Debug)]
enum Mudanca {
    /// Lista nova de monitores (Windows: um monitor ligado ou desligado).
    #[cfg_attr(not(windows), allow(dead_code))]
    Monitores(Vec<DisplayInfo>),
    /// O compositor encerrou o stream deste monitor (Wayland: monitor desligado).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    StreamFechou(u32),
}

/// De quanto em quanto tempo o host confere a lista de monitores (Windows).
#[cfg(windows)]
const INTERVALO_MONITORES: Duration = Duration::from_secs(2);

/// Confere a lista de monitores e avisa quando ela muda (Windows; DXGI não tem evento).
#[cfg(windows)]
async fn vigiar_monitores(mut atual: Vec<DisplayInfo>, tx: mpsc::UnboundedSender<Mudanca>) {
    let mut t = tokio::time::interval(INTERVALO_MONITORES);
    t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    t.tick().await;
    loop {
        t.tick().await;
        let Ok(lista) = tokio::task::spawn_blocking(crate::host::monitores).await else {
            continue;
        };
        // Lista vazia é passageira (troca de modo, tela de bloqueio): espera a próxima.
        if lista.is_empty() || lista == atual {
            continue;
        }
        atual.clone_from(&lista);
        if tx.send(Mudanca::Monitores(lista)).is_err() {
            return;
        }
    }
}

/// Captura de todos os monitores, com pausa por monitor.
enum Captura {
    #[cfg(windows)]
    Dxgi(taildesk_platform::captura_dxgi::CapturaDxgi),
    #[cfg(target_os = "linux")]
    PipeWire(taildesk_platform::captura_pipewire::CapturaPipeWire),
    #[cfg(test)]
    // Só o Drop (parar a thread) importa.
    Sintetica(#[allow(dead_code)] tests::Sintetica),
}

impl Captura {
    fn ativar(&self, display_id: u32, ativo: bool) {
        match self {
            #[cfg(windows)]
            Captura::Dxgi(c) => c.ativar(display_id, ativo),
            #[cfg(target_os = "linux")]
            Captura::PipeWire(c) => c.ativar(display_id, ativo),
            #[cfg(test)]
            Captura::Sintetica(_) => {}
        }
    }
}

/// De onde a captura sai.
#[derive(Clone)]
pub enum Fonte {
    #[cfg(windows)]
    Dxgi,
    /// Sessão do portal (a mesma do mouse e do teclado).
    #[cfg(target_os = "linux")]
    Portal(Arc<taildesk_platform::wayland::SessaoPortal>),
    /// Sem captura (portal recusado, sistema sem suporte): todo `StartVideo` é recusado.
    Nenhuma,
    /// Quadros e cursor gerados, para os testes no container.
    #[cfg(test)]
    Sintetica,
}

/// Vídeo de uma sessão do host.
pub struct MidiaApp {
    canal: CanalMidia,
    fonte: Fonte,
    monitores: Vec<DisplayInfo>,
    compartilhado: Arc<Compartilhado>,
    captura: Option<Captura>,
    stream_id: HashMap<u32, u8>,
    proximo_stream: u8,
    /// Codec e taxa pedidos para cada monitor (`StartVideo.wanted` e `SetVideoQuality`).
    pedidos: HashMap<u32, QualityPrefs>,
    /// Prioridade de cada monitor (`StartVideo.priority` e `SetPriority`).
    prioridades: HashMap<u32, Priority>,
    tarefa_cursor: tokio::task::JoinHandle<()>,
    tarefa_stats: tokio::task::JoinHandle<()>,
    tarefa_taxa: tokio::task::JoinHandle<()>,
    /// Som do sistema saindo (`stream_id`, captura), só com `Permissions.audio` e `StartAudio`.
    som: Option<(u32, SomHost)>,
    /// Mudanças da máquina (monitor ligado ou desligado) esperando [`Midia::tratar_evento`].
    /// No Windows só o vigia manda (com a cópia dele).
    #[cfg_attr(windows, allow(dead_code))]
    mudancas_tx: mpsc::UnboundedSender<Mudanca>,
    mudancas: mpsc::UnboundedReceiver<Mudanca>,
    pendentes: Vec<Mudanca>,
    tarefa_vigia: Option<tokio::task::JoinHandle<()>>,
}

impl MidiaApp {
    pub fn new(canal: CanalMidia, fonte: Fonte, monitores: Vec<DisplayInfo>) -> MidiaApp {
        let (tx, rx) = mpsc::channel(FILA_CURSOR);
        let tarefa_cursor = tokio::spawn(cursor(canal.clone(), rx));
        let compartilhado = Arc::new(Compartilhado {
            fluxos: Mutex::new(HashMap::new()),
            cursor: tx,
            relato: Mutex::new(None),
        });
        let tarefa_stats = tokio::spawn(stats(canal.clone(), compartilhado.clone()));
        let tarefa_taxa = tokio::spawn(controle_taxa(canal.clone(), compartilhado.clone()));
        let (mudancas_tx, mudancas) = mpsc::unbounded_channel();
        #[cfg(windows)]
        let tarefa_vigia = matches!(fonte, Fonte::Dxgi)
            .then(|| tokio::spawn(vigiar_monitores(monitores.clone(), mudancas_tx.clone())));
        #[cfg(not(windows))]
        let tarefa_vigia = None;
        MidiaApp {
            canal,
            fonte,
            monitores,
            compartilhado,
            captura: None,
            stream_id: HashMap::new(),
            proximo_stream: 0,
            pedidos: HashMap::new(),
            prioridades: HashMap::new(),
            tarefa_cursor,
            tarefa_stats,
            tarefa_taxa,
            som: None,
            mudancas_tx,
            mudancas,
            pendentes: Vec::new(),
            tarefa_vigia,
        }
    }

    fn ao_quadro(&self) -> AoQuadro {
        AoQuadro {
            compartilhado: self.compartilhado.clone(),
            tamanhos: self
                .monitores
                .iter()
                .map(|m| (m.id, (m.width, m.height)))
                .collect(),
        }
    }

    async fn abrir_captura(&self) -> Result<Captura, String> {
        match &self.fonte {
            #[cfg(windows)]
            Fonte::Dxgi => {
                use taildesk_platform::captura_dxgi::{CapturaDxgi, listar_monitores};
                let monitores = listar_monitores().map_err(|e| e.to_string())?;
                let mut cb = self.ao_quadro();
                let c = tokio::task::spawn_blocking(move || {
                    CapturaDxgi::iniciar(&monitores, move |q: &Quadro<'_>| cb.quadro(q))
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
                Ok(Captura::Dxgi(c))
            }
            #[cfg(target_os = "linux")]
            Fonte::Portal(portal) => {
                use taildesk_platform::captura_pipewire::{CapturaPipeWire, Conexao};
                let fd = portal.abrir_pipewire().await.map_err(|e| e.to_string())?;
                let nodes: Vec<(u32, u32)> = portal
                    .monitores()
                    .iter()
                    .map(|m| (m.display_id, m.node_id))
                    .collect();
                let mut cb = self.ao_quadro();
                let tx = self.mudancas_tx.clone();
                let c = tokio::task::spawn_blocking(move || {
                    CapturaPipeWire::iniciar(
                        Conexao::Portal(fd),
                        &nodes,
                        move |q: &Quadro<'_>| cb.quadro(q),
                        move |id| {
                            let _ = tx.send(Mudanca::StreamFechou(id));
                        },
                    )
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
                Ok(Captura::PipeWire(c))
            }
            Fonte::Nenhuma => Err("captura de tela indisponível".into()),
            #[cfg(test)]
            Fonte::Sintetica => Ok(Captura::Sintetica(tests::Sintetica::iniciar(
                self.ao_quadro(),
                self.monitores
                    .iter()
                    .map(|m| (m.id, m.width, m.height))
                    .collect(),
            ))),
        }
    }

    async fn erro(&self, code: ErrorCode, display_id: u32, message: String) {
        self.canal
            .enviar(Msg::Error(Error {
                code: code.into(),
                display_id: Some(display_id),
                message,
            }))
            .await;
    }

    async fn parou(&self, display_id: u32, motivo: VideoStopReason) {
        self.canal
            .enviar(Msg::VideoStopped(VideoStopped {
                display_id,
                reason: motivo.into(),
            }))
            .await;
    }

    async fn iniciar(&mut self, s: &StartVideo) {
        let id = s.display_id;
        let prefs = s.wanted.unwrap_or_default();
        let Some(m) = self.monitores.iter().find(|m| m.id == id).cloned() else {
            return self
                .erro(ErrorCode::UnknownDisplay, id, String::new())
                .await;
        };
        if !m.shared {
            return self
                .erro(ErrorCode::DisplayNotShared, id, String::new())
                .await;
        }
        self.prioridade(id, s.priority);
        if let Some(f) = self.compartilhado.fluxos().get(&id) {
            // Já está saindo (o controlador reabriu a aba): só um quadro-chave novo.
            f.envio.pedir_chave();
            return;
        }
        if self.compartilhado.fluxos().len() >= MAX_FLUXOS {
            return self
                .erro(ErrorCode::TooManyStreams, id, String::new())
                .await;
        }
        if self.captura.is_none() {
            match self.abrir_captura().await {
                Ok(c) => {
                    // Ninguém pediu os outros ainda.
                    for outro in &self.monitores {
                        c.ativar(outro.id, false);
                    }
                    self.captura = Some(c);
                }
                Err(e) => {
                    eprintln!("taildesk: captura: {e}");
                    return self.parou(id, VideoStopReason::PortalClosed).await;
                }
            }
        }
        self.pedidos.insert(id, prefs);
        if self.abrir_fluxo(&m, prefs).await
            && let Some(c) = &self.captura
        {
            c.ativar(id, true);
        }
    }

    /// Abre o encoder e o envio de um monitor e manda o `CodecSelected`. `false` = não abriu (o
    /// controlador já recebeu o `VideoStopped`).
    async fn abrir_fluxo(&mut self, m: &DisplayInfo, prefs: QualityPrefs) -> bool {
        let id = m.id;
        // O codec escolhido, se houver; senão o melhor que o controlador decodifica, hardware
        // primeiro (NVENC, VA-API) e o openh264 por último. O probe codifica um quadro.
        let aceitos = codecs_aceitos(&self.canal.decode_caps);
        let pedido = codec_video(prefs.codec);
        let taxa_fixa = taxa_pedida(&prefs);
        let bitrate_bps = taxa_fixa.unwrap_or(BITRATE_BPS);
        let prioridade = self
            .prioridades
            .get(&id)
            .copied()
            .unwrap_or(Priority::Focused);
        let so_hardware = self.compartilhado.fluxos().len() >= MAX_FLUXOS_SOFTWARE;
        let modo = modo(&prefs);
        let encoder = tokio::task::spawn_blocking(move || {
            for opcao in candidatos(&aceitos, pedido) {
                if so_hardware && !opcao.hardware() {
                    continue;
                }
                let fps = if opcao.hardware() {
                    FPS_HARDWARE
                } else {
                    FPS_SOFTWARE
                };
                let config = ConfigEncoder {
                    bitrate_bps,
                    fps_max: fps,
                };
                match opcao.abrir_encoder(config) {
                    Ok(enc) => return Ok((enc, fps)),
                    Err(e) => eprintln!(
                        "taildesk: encoder {} {} ({}) indisponível: {e}",
                        taildesk_codec::nome_codec(opcao.codec),
                        opcao.backend,
                        opcao.gpu.as_deref().unwrap_or("CPU")
                    ),
                }
            }
            Err("nenhum encoder de vídeo abriu".to_string())
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
        let mut fps = FPS_SOFTWARE;
        let mut codec = CodecVideo::H264;
        let envio = encoder.and_then(|(enc, f)| {
            fps = f;
            let (nome, gpu) = (enc.nome(), enc.gpu().unwrap_or_default().to_string());
            codec = enc.codec();
            eprintln!(
                "taildesk: vídeo do monitor {id}: {} em {nome} ({})",
                taildesk_codec::nome_codec(codec),
                if gpu.is_empty() { "CPU" } else { &gpu }
            );
            let stream_id = self.proximo_stream;
            EnvioVideo::iniciar(
                ConfigEnvio {
                    stream_id,
                    fps_max: fps,
                },
                enc,
                Box::new(SaidaQuic(self.canal.conn.clone())),
            )
            .map(|e| {
                // O envio passa a saber a taxa (o tamanho máximo da fila depende dela); igual à
                // do encoder, não muda nada nele.
                e.ajustar_bitrate(bitrate_bps);
                let fluxo = Fluxo {
                    anterior: (Instant::now(), Amostra::default()),
                    envio: e,
                    encoder: nome,
                    gpu,
                    codec,
                    bitrate_bps,
                    teto_bps: bitrate_bps,
                    taxa_usuario: taxa_fixa,
                    fps,
                    fps_usuario: fps_pedido(&prefs, fps),
                    modo,
                    escada: Escada::new(modo, fps_pedido(&prefs, fps)),
                    prioridade,
                    visivel: false,
                    ultima_subida: None,
                };
                fluxo.aplicar_degrau();
                (stream_id, fluxo)
            })
            .map_err(|e| e.to_string())
        });
        let (stream_id, fluxo) = match envio {
            Ok(v) => v,
            Err(e) if so_hardware => {
                // Em software já saem 2 monitores; sem hardware livre, não cabe mais um.
                eprintln!("taildesk: encoder: {e}");
                self.erro(ErrorCode::TooManyStreams, id, String::new())
                    .await;
                return false;
            }
            Err(e) => {
                eprintln!("taildesk: encoder: {e}");
                self.parou(id, VideoStopReason::EncoderError).await;
                return false;
            }
        };
        self.proximo_stream = (self.proximo_stream + 1) % AUDIO_STREAM_MIN;
        // O `CodecSelected` vai antes dos datagramas, para o controlador já ter a recepção.
        self.canal
            .enviar(Msg::CodecSelected(CodecSelected {
                display_id: id,
                stream_id: stream_id.into(),
                codec: codec_proto(codec).into(),
                profile: 0,
                chroma444: false,
                width: m.width,
                height: m.height,
                fps_target: fps as u32,
                bitrate_kbps: bitrate_bps / 1000,
                // BT.709, faixa limitada (o que o openh264 produz).
                color: Some(ColorInfo {
                    primaries: 1,
                    transfer: 1,
                    matrix: 1,
                    full_range: false,
                }),
            }))
            .await;
        self.stream_id.insert(id, stream_id);
        self.compartilhado.fluxos().insert(id, fluxo);
        true
    }

    /// Seletor da sessão: codec novo reabre o encoder (outro `CodecSelected`); só a taxa muda a
    /// quente.
    async fn qualidade(&mut self, q: &SetVideoQuality) {
        let id = q.display_id;
        let prefs = q.wanted.unwrap_or_default();
        let antes = self.pedidos.insert(id, prefs).unwrap_or_default();
        let Some(m) = self.monitores.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let Some(atual) = self.compartilhado.fluxos().get(&id).map(|f| f.codec) else {
            // Ainda não está saindo: vale no próximo `StartVideo`.
            return;
        };
        // Pedir o codec que já está saindo não reabre nada.
        let reabrir =
            antes.codec != prefs.codec && codec_video(prefs.codec).is_none_or(|c| c != atual);
        if reabrir {
            let envio = self.compartilhado.fluxos().remove(&id).map(|f| f.envio);
            self.stream_id.remove(&id);
            if let Some(e) = envio {
                let _ = tokio::task::spawn_blocking(move || drop(e)).await;
            }
            if !self.abrir_fluxo(&m, prefs).await {
                self.pedidos.remove(&id);
                if let Some(c) = &self.captura {
                    c.ativar(id, false);
                }
            }
            return;
        }
        if let Some(f) = self.compartilhado.fluxos().get_mut(&id) {
            f.modo = modo(&prefs);
            f.fps_usuario = fps_pedido(&prefs, f.fps);
            f.taxa_usuario = taxa_pedida(&prefs);
            f.preferir(Instant::now());
            let alvo = f.teto_desejado().unwrap_or(BITRATE_BPS);
            // O teto muda na hora; o próximo passo do controle de taxa aplica o fator.
            f.teto_bps = alvo;
            f.envio.ajustar_bitrate(alvo);
            f.bitrate_bps = alvo;
        }
    }

    async fn parar(&mut self, display_id: u32) {
        let envio = self
            .compartilhado
            .fluxos()
            .remove(&display_id)
            .map(|f| f.envio);
        self.stream_id.remove(&display_id);
        self.pedidos.remove(&display_id);
        self.prioridades.remove(&display_id);
        if let Some(c) = &self.captura {
            c.ativar(display_id, false);
        }
        if let Some(e) = envio {
            // Soltar espera a thread do encode terminar o quadro atual.
            let _ = tokio::task::spawn_blocking(move || drop(e)).await;
        }
        // Sem ninguém vendo, a captura fecha (DXGI e PipeWire custam mesmo pausados).
        if self.compartilhado.fluxos().is_empty()
            && let Some(c) = self.captura.take()
        {
            let _ = tokio::task::spawn_blocking(move || drop(c)).await;
        }
        self.parou(display_id, VideoStopReason::ByController).await;
    }

    /// `StartVideo.priority` ou `SetPriority`. O laço de taxa aplica em até 100 ms.
    fn prioridade(&mut self, display_id: u32, p: i32) {
        let p = match Priority::try_from(p) {
            Ok(Priority::Visible) => Priority::Visible,
            _ => Priority::Focused,
        };
        self.prioridades.insert(display_id, p);
        if let Some(f) = self.compartilhado.fluxos().get_mut(&display_id) {
            f.prioridade = p;
        }
    }

    /// Para o vídeo de um monitor que sumiu, sem esperar o controlador pedir.
    async fn monitor_saiu(&mut self, display_id: u32) {
        let envio = self
            .compartilhado
            .fluxos()
            .remove(&display_id)
            .map(|f| f.envio);
        self.stream_id.remove(&display_id);
        self.pedidos.remove(&display_id);
        self.prioridades.remove(&display_id);
        if let Some(e) = envio {
            let _ = tokio::task::spawn_blocking(move || drop(e)).await;
            self.parou(display_id, VideoStopReason::DisplayRemoved)
                .await;
        }
    }

    /// Lista nova de monitores: para o vídeo dos que saíram, reabre a captura com a lista nova
    /// (os que continuam seguem no mesmo stream, com um quadro-chave) e avisa o controlador.
    async fn monitores_mudaram(&mut self, lista: Vec<DisplayInfo>) {
        let sairam: Vec<u32> = self
            .monitores
            .iter()
            .map(|m| m.id)
            .filter(|id| !lista.iter().any(|m| m.id == *id))
            .collect();
        eprintln!(
            "taildesk: monitores mudaram ({} agora, {} saíram)",
            lista.len(),
            sairam.len()
        );
        for id in sairam {
            self.monitor_saiu(id).await;
        }
        self.monitores = lista;
        if let Some(c) = self.captura.take() {
            let _ = tokio::task::spawn_blocking(move || drop(c)).await;
        }
        let saindo: Vec<u32> = self.compartilhado.fluxos().keys().copied().collect();
        if !saindo.is_empty() {
            match self.abrir_captura().await {
                Ok(c) => {
                    for m in &self.monitores {
                        c.ativar(m.id, saindo.contains(&m.id));
                    }
                    self.captura = Some(c);
                    for id in &saindo {
                        self.pedir_chave(*id);
                    }
                }
                Err(e) => {
                    eprintln!("taildesk: captura: {e}");
                    for id in saindo {
                        self.monitor_saiu(id).await;
                    }
                }
            }
        }
        self.canal
            .enviar(Msg::DisplaysChanged(DisplaysChanged {
                displays: self.monitores.clone(),
            }))
            .await;
    }

    /// O compositor fechou o stream de um monitor (Wayland): ele sai da lista.
    async fn stream_fechou(&mut self, display_id: u32) {
        if !self.monitores.iter().any(|m| m.id == display_id) {
            return;
        }
        eprintln!("taildesk: o stream do monitor {display_id} foi encerrado pelo sistema");
        self.monitor_saiu(display_id).await;
        self.monitores.retain(|m| m.id != display_id);
        self.canal
            .enviar(Msg::DisplaysChanged(DisplaysChanged {
                displays: self.monitores.clone(),
            }))
            .await;
    }

    fn pedir_chave(&self, display_id: u32) {
        if let Some(f) = self.compartilhado.fluxos().get(&display_id) {
            f.envio.pedir_chave();
        }
    }
}

impl MidiaApp {
    /// `StartAudio`: abre a captura e responde `AudioSelected` (ou `AudioStopped` se o
    /// dispositivo não abrir).
    async fn iniciar_som(&mut self, a: &StartAudio) {
        let Ok(stream) = u8::try_from(a.stream_id) else {
            return;
        };
        if self.som.as_ref().is_some_and(|(id, _)| *id != a.stream_id) {
            self.parar_som(AudioStopReason::ByController).await;
        }
        if self.som.is_none() {
            let conn = self.canal.conn.clone();
            let aberto = tokio::task::spawn_blocking(move || SomHost::iniciar(conn, stream))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            match aberto {
                Ok(s) => self.som = Some((a.stream_id, s)),
                Err(e) => {
                    eprintln!("taildesk: som do sistema indisponível: {e}");
                    self.canal
                        .enviar(Msg::AudioStopped(AudioStopped {
                            stream_id: a.stream_id,
                            reason: AudioStopReason::DeviceError.into(),
                        }))
                        .await;
                    return;
                }
            }
        }
        self.canal
            .enviar(Msg::AudioSelected(AudioSelected {
                stream_id: a.stream_id,
                codec: AudioCodec::Opus.into(),
                sample_rate: taildesk_media::audio::TAXA,
                channels: taildesk_media::audio::CANAIS as u32,
                frame_ms_x10: taildesk_media::audio::QUADRO_MS_X10,
                bitrate_kbps: taildesk_media::audio::BITRATE_KBPS,
                redundancy: false,
                inband_fec: false,
            }))
            .await;
    }

    /// Fecha a captura (e espera o encoder parar) antes de avisar: depois do `AudioStopped`
    /// nenhum `kind=1` sai (`15-audio.md` §4.3).
    async fn parar_som(&mut self, motivo: AudioStopReason) {
        let Some((stream_id, s)) = self.som.take() else {
            return;
        };
        let _ = tokio::task::spawn_blocking(move || drop(s)).await;
        self.canal
            .enviar(Msg::AudioStopped(AudioStopped {
                stream_id,
                reason: motivo.into(),
            }))
            .await;
    }
}

impl Midia for MidiaApp {
    async fn mensagem(&mut self, msg: &Msg) {
        match msg {
            Msg::StartVideo(s) => self.iniciar(s).await,
            Msg::StopVideo(s) => self.parar(s.display_id).await,
            Msg::SetVideoQuality(q) => self.qualidade(q).await,
            Msg::RequestKeyframe(r) => self.pedir_chave(r.display_id),
            Msg::PictureLoss(p) => self.pedir_chave(p.display_id),
            Msg::StartAudio(a) => self.iniciar_som(a).await,
            Msg::StopAudio(_) => self.parar_som(AudioStopReason::ByController).await,
            Msg::ReceiverReport(r) => {
                *self
                    .compartilhado
                    .relato
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some((
                    Instant::now(),
                    Relato {
                        loss_ppm: r.loss_ppm,
                        owd_trend_us: r.owd_trend_us,
                    },
                ));
            }
            Msg::SetPriority(p) => self.prioridade(p.display_id, p.priority),
            _ => {}
        }
    }

    async fn evento(&mut self) {
        // Seguro de cancelar: `recv` é, e guardar o que chegou não espera nada.
        match self.mudancas.recv().await {
            Some(m) => self.pendentes.push(m),
            None => std::future::pending().await,
        }
    }

    async fn tratar_evento(&mut self) {
        for m in std::mem::take(&mut self.pendentes) {
            match m {
                Mudanca::Monitores(lista) => self.monitores_mudaram(lista).await,
                Mudanca::StreamFechou(id) => self.stream_fechou(id).await,
            }
        }
    }
}

impl Drop for MidiaApp {
    fn drop(&mut self) {
        // A captura primeiro (para de entregar), depois os envios (juntam as threads).
        self.captura = None;
        self.som = None;
        self.compartilhado.fluxos().clear();
        self.tarefa_cursor.abort();
        self.tarefa_stats.abort();
        self.tarefa_taxa.abort();
        if let Some(t) = &self.tarefa_vigia {
            t.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    use taildesk_codec::DecoderOpenH264;
    use taildesk_media::video::RecepcaoVideo;
    use taildesk_platform::captura::{Cursor, Imagem, Rotacao};
    use taildesk_proto::control::{Control, SetPriority, StopVideo};
    use taildesk_proto::cursor::limits;
    use taildesk_transport::{Identity, Pin, client_endpoint, connect, server_endpoint};

    use crate::video::{Recepcoes, ler_datagramas, travar};

    /// Captura de mentira: quadros BGRx que mudam e um cursor 2×2, a cada 10 ms.
    pub struct Sintetica {
        parar: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Sintetica {
        pub fn iniciar(mut cb: AoQuadro, monitores: Vec<(u32, u32, u32)>) -> Sintetica {
            let parar = Arc::new(AtomicBool::new(false));
            let p = parar.clone();
            let thread = std::thread::spawn(move || {
                let mut n = 0u32;
                let forma = [255u8; 16];
                while !p.load(Ordering::Relaxed) {
                    for &(id, l, a) in &monitores {
                        let dados: Vec<u8> =
                            (0..l * a * 4).map(|i| (i / 4 + n * 3) as u8).collect();
                        cb.quadro(&Quadro {
                            display_id: id,
                            imagem: Some(Imagem {
                                largura: l,
                                altura: a,
                                stride: l as usize * 4,
                                formato: FormatoPixel::Bgrx,
                                rotacao: Rotacao::Nenhuma,
                                dados: &dados,
                            }),
                            cursor: Some(Cursor {
                                visivel: true,
                                x: (n % l) as i32,
                                y: 5,
                                forma: Some(FormaCursor {
                                    largura: 2,
                                    altura: 2,
                                    stride: 8,
                                    hotspot_x: 0,
                                    hotspot_y: 0,
                                    formato: FormatoPixel::Bgra,
                                    dados: &forma,
                                }),
                            }),
                        });
                    }
                    n += 1;
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            Sintetica {
                parar,
                thread: Some(thread),
            }
        }
    }

    impl Drop for Sintetica {
        fn drop(&mut self) {
            self.parar.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    async fn proxima(rx: &mut mpsc::Receiver<Control>, quer: fn(&Msg) -> bool) -> Msg {
        loop {
            let c = tokio::time::timeout(Duration::from_secs(20), rx.recv())
                .await
                .expect("mensagem demorou")
                .expect("canal fechou");
            if let Some(m) = c.msg
                && quer(&m)
            {
                return m;
            }
        }
    }

    /// Host do app (captura → openh264 → QUIC) até a recepção do controlador, em localhost.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn video_e_cursor_de_ponta_a_ponta() {
        let id = Identity::generate().unwrap();
        let server = server_endpoint((Ipv4Addr::LOCALHOST, 0).into(), &id).unwrap();
        let addr = server.local_addr().unwrap();
        let aceitar = tokio::spawn(async move { server.accept().await.unwrap().await.unwrap() });
        let client = client_endpoint(addr).unwrap();
        let controlador = connect(&client, addr, Pin::Sha256(id.fingerprint()))
            .await
            .unwrap();
        let host = aceitar.await.unwrap();

        let (saida, mut rx) = mpsc::channel(1024);
        let monitor = DisplayInfo {
            id: 3,
            width: 160,
            height: 96,
            shared: true,
            ..DisplayInfo::default()
        };
        let mut midia = MidiaApp::new(
            CanalMidia {
                conn: host,
                saida,
                decode_caps: Vec::new(),
            },
            Fonte::Sintetica,
            vec![monitor],
        );
        midia
            .mensagem(&Msg::StartVideo(StartVideo {
                display_id: 3,
                ..StartVideo::default()
            }))
            .await;
        let Msg::CodecSelected(c) = proxima(&mut rx, |m| matches!(m, Msg::CodecSelected(_))).await
        else {
            unreachable!()
        };
        assert_eq!((c.display_id, c.width, c.height), (3, 160, 96));

        // Controlador: recepção pelo `stream_id` do `CodecSelected`, como a janela faz.
        let decodificados = Arc::new(AtomicU64::new(0));
        let recepcoes = Recepcoes::default();
        let r = RecepcaoVideo::iniciar(
            3,
            c.stream_id as u8,
            Box::new(DecoderOpenH264::novo().unwrap()),
            Box::new({
                let d = decodificados.clone();
                move |display, q| {
                    assert_eq!((display, q.largura, q.altura), (3, 160, 96));
                    d.fetch_add(1, Ordering::Relaxed);
                }
            }),
            Arc::new(|| {}),
        )
        .unwrap();
        travar(&recepcoes).insert(c.stream_id as u8, (3, r));
        let contadores = Arc::new(crate::video::Contadores::default());
        let leitor = tokio::spawn(ler_datagramas(
            controlador.clone(),
            recepcoes.clone(),
            contadores.clone(),
            crate::som::Destino::default(),
        ));
        for _ in 0..400 {
            if decodificados.load(Ordering::Relaxed) >= 5 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(decodificados.load(Ordering::Relaxed) >= 5);
        assert!(contadores.bytes.load(Ordering::Relaxed) > 0);

        // Cursor: a forma no stream de cursor, antes do `CursorShapeRef`; a posição no controle.
        let Msg::CursorShapeRef(r) =
            proxima(&mut rx, |m| matches!(m, Msg::CursorShapeRef(_))).await
        else {
            unreachable!()
        };
        let mut uni = controlador.accept_uni().await.unwrap();
        let mut cab = [0u8; 4 + ShapeHeader::LEN];
        uni.read_exact(&mut cab).await.unwrap();
        assert_eq!(cab[..4], STREAM_MAGIC);
        let h = ShapeHeader::decode(cab[4..].try_into().unwrap()).unwrap();
        assert_eq!((h.hash, h.width, h.height), (r.hash, 2, 2));
        let Msg::CursorPos(p) = proxima(&mut rx, |m| matches!(m, Msg::CursorPos(_))).await else {
            unreachable!()
        };
        assert!(p.visible && p.display_id == 3 && p.y == u32::from(normalizar(5, 96)));
        // Estatísticas para o painel de diagnóstico: encoder escolhido e quadros saindo.
        let Msg::VideoStats(st) = proxima(&mut rx, |m| matches!(m, Msg::VideoStats(_))).await
        else {
            unreachable!()
        };
        eprintln!("encoder escolhido: {} ({:?})", st.encoder, st.gpu);
        assert_eq!(st.display_id, 3);
        assert!(!st.encoder.is_empty());

        // Parar confirma com `VideoStopped` e solta a captura.
        midia
            .mensagem(&Msg::StopVideo(StopVideo { display_id: 3 }))
            .await;
        let Msg::VideoStopped(v) = proxima(&mut rx, |m| matches!(m, Msg::VideoStopped(_))).await
        else {
            unreachable!()
        };
        assert_eq!(v.display_id, 3);
        assert!(midia.captura.is_none());
        drop(midia);
        leitor.abort();
    }

    /// Seletor da sessão: taxa fixa no `StartVideo`, volta ao automático a quente e troca de
    /// codec reabre o encoder com outro `CodecSelected` (aqui cai no H.264, o que o
    /// controlador decodifica).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn seletor_de_codec_e_taxa() {
        let id = Identity::generate().unwrap();
        let server = server_endpoint((Ipv4Addr::LOCALHOST, 0).into(), &id).unwrap();
        let addr = server.local_addr().unwrap();
        let aceitar = tokio::spawn(async move { server.accept().await.unwrap().await.unwrap() });
        let client = client_endpoint(addr).unwrap();
        let _controlador = connect(&client, addr, Pin::Sha256(id.fingerprint()))
            .await
            .unwrap();
        let host = aceitar.await.unwrap();
        let (saida, mut rx) = mpsc::channel(1024);
        let monitor = DisplayInfo {
            id: 1,
            width: 160,
            height: 96,
            shared: true,
            ..DisplayInfo::default()
        };
        let mut midia = MidiaApp::new(
            CanalMidia {
                conn: host,
                saida,
                decode_caps: Vec::new(),
            },
            Fonte::Sintetica,
            vec![monitor],
        );
        let prefs = |codec: Codec, kbps| {
            Some(QualityPrefs {
                codec: codec.into(),
                bitrate_kbps: kbps,
                ..QualityPrefs::default()
            })
        };
        midia
            .mensagem(&Msg::StartVideo(StartVideo {
                display_id: 1,
                wanted: prefs(Codec::Unspecified, 5_000),
                ..StartVideo::default()
            }))
            .await;
        let Msg::CodecSelected(c) = proxima(&mut rx, |m| matches!(m, Msg::CodecSelected(_))).await
        else {
            unreachable!()
        };
        assert_eq!(c.bitrate_kbps, 5_000);
        // Taxa fixa: o ajuste pelo tamanho (160x96 dá o piso de 4 Mbit/s) não mexe nela.
        let Msg::VideoStats(st) = proxima(&mut rx, |m| matches!(m, Msg::VideoStats(_))).await
        else {
            unreachable!()
        };
        assert_eq!(st.target_kbps, 5_000);

        // De volta ao automático, sem reabrir nada.
        midia
            .mensagem(&Msg::SetVideoQuality(SetVideoQuality {
                display_id: 1,
                wanted: prefs(Codec::Unspecified, 0),
            }))
            .await;
        // Uma amostra de antes da troca ainda pode estar na fila.
        let mut alvo = 0;
        for _ in 0..3 {
            let Msg::VideoStats(st) = proxima(&mut rx, |m| {
                matches!(m, Msg::VideoStats(_) | Msg::CodecSelected(_))
            })
            .await
            else {
                panic!("taxa nova não pode reabrir o encoder");
            };
            alvo = st.target_kbps;
            if alvo != 5_000 {
                break;
            }
        }
        assert_eq!(alvo, 4_000);

        // Codec novo: encoder novo e outro stream. O controlador só decodifica H.264.
        midia
            .mensagem(&Msg::SetVideoQuality(SetVideoQuality {
                display_id: 1,
                wanted: prefs(Codec::Hevc, 0),
            }))
            .await;
        let Msg::CodecSelected(n) = proxima(&mut rx, |m| {
            matches!(m, Msg::CodecSelected(_) | Msg::VideoStopped(_))
        })
        .await
        else {
            panic!("trocar de codec não pode parar o vídeo");
        };
        assert_eq!(n.codec(), Codec::H264);
        assert_ne!(n.stream_id, c.stream_id);
        assert!(midia.captura.is_some());
        drop(midia);
    }

    /// Dois monitores: o `Visible` sai com no máximo 15 fps e menos taxa enquanto o outro é
    /// `Focused`; trocar o foco inverte.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prioridade_visible_corta_fps_e_taxa() {
        let id = Identity::generate().unwrap();
        let server = server_endpoint((Ipv4Addr::LOCALHOST, 0).into(), &id).unwrap();
        let addr = server.local_addr().unwrap();
        let aceitar = tokio::spawn(async move { server.accept().await.unwrap().await.unwrap() });
        let client = client_endpoint(addr).unwrap();
        let _controlador = connect(&client, addr, Pin::Sha256(id.fingerprint()))
            .await
            .unwrap();
        let host = aceitar.await.unwrap();
        let (saida, mut rx) = mpsc::channel(1024);
        let monitor = |id| DisplayInfo {
            id,
            width: 160,
            height: 96,
            shared: true,
            ..DisplayInfo::default()
        };
        let mut midia = MidiaApp::new(
            CanalMidia {
                conn: host,
                saida,
                decode_caps: Vec::new(),
            },
            Fonte::Sintetica,
            vec![monitor(1), monitor(2)],
        );
        for (d, p) in [(1, Priority::Focused), (2, Priority::Visible)] {
            midia
                .mensagem(&Msg::StartVideo(StartVideo {
                    display_id: d,
                    priority: p.into(),
                    ..StartVideo::default()
                }))
                .await;
        }
        /// Próximo `VideoStats` do monitor `d` que satisfaz `cond`.
        async fn stats(
            rx: &mut mpsc::Receiver<Control>,
            d: u32,
            cond: impl Fn(&VideoStats) -> bool,
        ) -> VideoStats {
            for _ in 0..20 {
                let Msg::VideoStats(st) = proxima(rx, |m| matches!(m, Msg::VideoStats(_))).await
                else {
                    unreachable!()
                };
                if st.display_id == d && cond(&st) {
                    return st;
                }
            }
            panic!("monitor {d} não chegou ao estado esperado");
        }
        // 160x96 fica no piso automático de 4 Mbit/s; o `Visible` leva 30 % disso.
        stats(&mut rx, 2, |s| s.fps_max == 15 && s.target_kbps == 1_200).await;
        stats(&mut rx, 1, |s| s.fps_max > 15 && s.target_kbps == 4_000).await;
        // Foco vai para o 2.
        for (d, p) in [(2, Priority::Focused), (1, Priority::Visible)] {
            midia
                .mensagem(&Msg::SetPriority(SetPriority {
                    display_id: d,
                    priority: p.into(),
                }))
                .await;
        }
        stats(&mut rx, 1, |s| s.fps_max == 15 && s.target_kbps == 1_200).await;
        stats(&mut rx, 2, |s| s.fps_max > 15 && s.target_kbps == 4_000).await;
        // Sem o outro saindo, o `Visible` não perde nada.
        midia
            .mensagem(&Msg::StopVideo(StopVideo { display_id: 2 }))
            .await;
        stats(&mut rx, 1, |s| s.fps_max > 15).await;
        drop(midia);
    }

    /// Monitor desligado no meio da sessão: o vídeo dele para com `DisplayRemoved`, o
    /// controlador recebe a lista nova e o outro monitor segue saindo.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn monitor_removido_e_ligado_no_meio_da_sessao() {
        let id = Identity::generate().unwrap();
        let server = server_endpoint((Ipv4Addr::LOCALHOST, 0).into(), &id).unwrap();
        let addr = server.local_addr().unwrap();
        let aceitar = tokio::spawn(async move { server.accept().await.unwrap().await.unwrap() });
        let client = client_endpoint(addr).unwrap();
        let _controlador = connect(&client, addr, Pin::Sha256(id.fingerprint()))
            .await
            .unwrap();
        let host = aceitar.await.unwrap();
        let (saida, mut rx) = mpsc::channel(1024);
        let monitor = |id| DisplayInfo {
            id,
            width: 160,
            height: 96,
            shared: true,
            ..DisplayInfo::default()
        };
        let mut midia = MidiaApp::new(
            CanalMidia {
                conn: host,
                saida,
                decode_caps: Vec::new(),
            },
            Fonte::Sintetica,
            vec![monitor(1), monitor(2)],
        );
        for d in [1, 2] {
            midia
                .mensagem(&Msg::StartVideo(StartVideo {
                    display_id: d,
                    ..StartVideo::default()
                }))
                .await;
            proxima(&mut rx, |m| matches!(m, Msg::CodecSelected(_))).await;
        }
        // Windows: a lista nova perde o 2 e ganha o 3.
        midia
            .mudancas_tx
            .send(Mudanca::Monitores(vec![monitor(1), monitor(3)]))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), midia.evento())
            .await
            .unwrap();
        midia.tratar_evento().await;
        let Msg::VideoStopped(v) = proxima(&mut rx, |m| matches!(m, Msg::VideoStopped(_))).await
        else {
            unreachable!()
        };
        assert_eq!(
            (v.display_id, v.reason()),
            (2, VideoStopReason::DisplayRemoved)
        );
        let Msg::DisplaysChanged(d) =
            proxima(&mut rx, |m| matches!(m, Msg::DisplaysChanged(_))).await
        else {
            unreachable!()
        };
        assert_eq!(d.displays.iter().map(|m| m.id).collect::<Vec<_>>(), [1, 3]);
        assert!(midia.captura.is_some());
        // O 1 continua saindo (sem `CodecSelected` novo) e o 3 pode ser pedido.
        let antes = midia.compartilhado.fluxos()[&1].amostra().quadros;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(midia.compartilhado.fluxos()[&1].amostra().quadros > antes);
        midia
            .mensagem(&Msg::StartVideo(StartVideo {
                display_id: 3,
                ..StartVideo::default()
            }))
            .await;
        let Msg::CodecSelected(c) = proxima(&mut rx, |m| matches!(m, Msg::CodecSelected(_))).await
        else {
            unreachable!()
        };
        assert_eq!(c.display_id, 3);

        // Wayland: o compositor fecha o stream do 1.
        midia.mudancas_tx.send(Mudanca::StreamFechou(1)).unwrap();
        midia.evento().await;
        midia.tratar_evento().await;
        let Msg::VideoStopped(v) = proxima(&mut rx, |m| matches!(m, Msg::VideoStopped(_))).await
        else {
            unreachable!()
        };
        assert_eq!(v.display_id, 1);
        let Msg::DisplaysChanged(d) =
            proxima(&mut rx, |m| matches!(m, Msg::DisplaysChanged(_))).await
        else {
            unreachable!()
        };
        assert_eq!(d.displays.iter().map(|m| m.id).collect::<Vec<_>>(), [3]);
        drop(midia);
    }

    #[test]
    fn perda_com_poucos_pacotes_nao_conta() {
        let mut p = Perda::default();
        // Tela parada: 5 pacotes por segundo, um perdido.
        assert_eq!(p.medir(5, 1), 0);
        // Vídeo: 1000 pacotes, 10 perdidos na janela (1 %).
        assert_eq!(p.medir(995, 9), 10_000);
        // A janela esquece o que passou de 5 intervalos.
        for _ in 0..5 {
            p.medir(1000, 0);
        }
        assert_eq!(p.medir(1000, 0), 0);
    }

    #[test]
    fn banda_pela_janela_de_congestionamento() {
        // 125 kB por 10 ms = 100 Mbit/s.
        assert_eq!(estimar_kbps(125_000, Duration::from_millis(10)), 100_000);
        assert_eq!(estimar_kbps(0, Duration::from_millis(10)), 0);
        assert!(estimar_kbps(u64::MAX, Duration::ZERO) == u32::MAX);
    }

    #[test]
    fn candidatos_poem_o_codec_pedido_na_frente() {
        // Sem decoder do outro lado, o pedido é ignorado e fica o automático (H.264).
        let auto = candidatos(&[], None);
        assert_eq!(candidatos(&[], Some(CodecVideo::Hevc)), auto);
        assert!(auto.iter().all(|o| o.codec == CodecVideo::H264));
        assert_eq!(auto.last().map(|o| o.backend), Some("openh264"));
    }

    #[test]
    fn cursor_normalizado_nos_cantos() {
        assert_eq!(normalizar(0, 1920), 0);
        assert_eq!(normalizar(1919, 1920), 65535);
        assert_eq!(normalizar(5000, 1920), 65535);
        assert_eq!(normalizar(-3, 1920), 0);
        assert_eq!(normalizar(0, 0), 0);
    }

    #[test]
    fn forma_em_rgba_com_hash_estavel() {
        // 2×1 BGRA com stride maior que a linha.
        let dados = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0];
        let f = FormaCursor {
            largura: 2,
            altura: 1,
            stride: 12,
            hotspot_x: 9,
            hotspot_y: -1,
            formato: FormatoPixel::Bgra,
            dados: &dados,
        };
        let (h, px) = forma_rgba(&f).unwrap();
        assert_eq!(px, [3, 2, 1, 4, 7, 6, 5, 8]);
        assert_eq!((h.hot_x, h.hot_y), (1, 0));
        assert_eq!(forma_rgba(&f).unwrap().0.hash, h.hash);
        let outra = FormaCursor {
            formato: FormatoPixel::Bgrx,
            ..f
        };
        assert_ne!(forma_rgba(&outra).unwrap().0.hash, h.hash);
        let grande = FormaCursor {
            largura: u32::from(limits::MAX_DIM) + 1,
            ..f
        };
        assert!(forma_rgba(&grande).is_none());
    }
}
