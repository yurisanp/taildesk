//! Mensagens do stream de controle (stream bidi 0), em protobuf com `prost`.
//!
//! Cada mensagem vai num [`Control`] cujo `oneof` tem uma faixa de tags por grupo
//! (`07-protocolo.md` §3). Faixas sem mensagem ainda estão reservadas para a fase que as traz:
//!
//! | Tags     | Grupo                                                    |
//! |----------|----------------------------------------------------------|
//! | 1–19     | Sessão                                                   |
//! | 20–39    | Vídeo, um stream por monitor (`13-multimonitor.md` §3)   |
//! | 40–49    | Mouse                                                    |
//! | 50–59    | Teclado (`11-teclado.md` §3, fase 2)                     |
//! | 60–69    | Cursor                                                   |
//! | 70–79    | Clipboard (fase 4)                                       |
//! | 80–89    | Arquivos (fase 3)                                        |
//! | 90–99    | QoS e seleção adaptativa (`14-selecao-adaptativa.md` §4) |
//! | 100–109  | Áudio (`15-audio.md` §6.5; reservado, sem implementação) |
//! | 110–119  | Chat                                                     |
//!
//! No fio cada [`Control`] vai precedido do tamanho em varint ([`encode_frame`], [`decode_frame`]).
//! Tag desconhecida no `oneof` chega como `msg: None`: quem recebe ignora e loga. Campos novos
//! são sempre opcionais; os números de tag e de enum abaixo nunca mudam de significado.

use std::fmt;

use prost::Message;

use crate::media::AUDIO_STREAM_MIN;

/// Versão do protocolo de controle anunciada no [`Hello`].
pub const PROTO_VERSION: u32 = 1;

/// Limites dos parsers (regra do `AGENTS.md`; `13-multimonitor.md` §3).
pub mod limits {
    /// Tamanho máximo de um [`super::Control`] codificado, sem o prefixo.
    pub const FRAME_LEN: usize = 64 * 1024;
    /// Monitores por mensagem.
    pub const DISPLAYS: usize = 16;
    /// Nome do monitor, em bytes.
    pub const DISPLAY_NAME_LEN: usize = 128;
    /// Textos curtos (versão, nome do host, sistema, arquitetura, modelo).
    pub const SHORT_TEXT_LEN: usize = 128;
    /// Mensagens livres (`Close`, `Error`).
    pub const MESSAGE_LEN: usize = 1024;
    /// Capacidades de codec por lista.
    pub const CODEC_CAPS: usize = 32;
    /// Recursos no `Hello`.
    pub const FEATURES: usize = 32;
    /// Bitmap de fragmentos de um `Nack`: `frag_count` cabe em u16, então 8 KiB cobre todos.
    pub const NACK_BITMAP_LEN: usize = 8 * 1024;
    /// Coordenada normalizada dentro do monitor (`MouseMove`, `CursorPos`).
    pub const COORD_MAX: u32 = u16::MAX as u32;
    /// Canais de áudio.
    pub const AUDIO_CHANNELS: u32 = 2;
}

// ---------------------------------------------------------------------------------------------
// Envelope

/// Uma mensagem do stream de controle.
#[derive(Clone, PartialEq, Message)]
pub struct Control {
    #[prost(
        oneof = "Msg",
        tags = "1, 2, 3, 4, 5, 6, 7, 20, 21, 22, 23, 24, 25, 26, 27, 28, 40, 41, 42, 60, 61, 100, 101, 102, 103, 104"
    )]
    pub msg: Option<Msg>,
}

/// Conteúdo de um [`Control`].
#[derive(Clone, PartialEq, prost::Oneof)]
pub enum Msg {
    // Sessão
    #[prost(message, tag = "1")]
    Hello(Hello),
    #[prost(message, tag = "2")]
    HelloAck(HelloAck),
    #[prost(message, tag = "3")]
    ConsentResult(ConsentResult),
    #[prost(message, tag = "4")]
    Close(Close),
    #[prost(message, tag = "5")]
    Ping(Ping),
    #[prost(message, tag = "6")]
    Pong(Pong),
    #[prost(message, tag = "7")]
    Error(Error),

    // Vídeo
    #[prost(message, tag = "20")]
    StartVideo(StartVideo),
    #[prost(message, tag = "21")]
    CodecSelected(CodecSelected),
    #[prost(message, tag = "22")]
    StopVideo(StopVideo),
    #[prost(message, tag = "23")]
    SetPriority(SetPriority),
    #[prost(message, tag = "24")]
    VideoStopped(VideoStopped),
    #[prost(message, tag = "25")]
    DisplaysChanged(DisplaysChanged),
    #[prost(message, tag = "26")]
    RequestKeyframe(RequestKeyframe),
    #[prost(message, tag = "27")]
    PictureLoss(PictureLoss),
    #[prost(message, tag = "28")]
    Nack(Nack),

    // Mouse
    #[prost(message, tag = "40")]
    MouseMove(MouseMove),
    #[prost(message, tag = "41")]
    MouseButton(MouseButton),
    #[prost(message, tag = "42")]
    Scroll(Scroll),

    // Cursor
    #[prost(message, tag = "60")]
    CursorPos(CursorPos),
    #[prost(message, tag = "61")]
    CursorShapeRef(CursorShapeRef),

    // Áudio (reservado: o host não captura nem envia nada sem `Permissions.audio` e um
    // `StartAudio` aceito, `15-audio.md` §6.4)
    #[prost(message, tag = "100")]
    StartAudio(StartAudio),
    #[prost(message, tag = "101")]
    AudioSelected(AudioSelected),
    #[prost(message, tag = "102")]
    AudioConfig(AudioConfig),
    #[prost(message, tag = "103")]
    StopAudio(StopAudio),
    #[prost(message, tag = "104")]
    AudioStopped(AudioStopped),
}

impl From<Msg> for Control {
    fn from(msg: Msg) -> Control {
        Control { msg: Some(msg) }
    }
}

// ---------------------------------------------------------------------------------------------
// Sessão

/// C→H: primeira mensagem. Só preferências; a identidade vem de `Overlay::identify()`.
#[derive(Clone, PartialEq, Message)]
pub struct Hello {
    #[prost(uint32, tag = "1")]
    pub proto_version: u32,
    #[prost(string, tag = "2")]
    pub app_version: String,
    #[prost(message, optional, tag = "3")]
    pub platform: Option<Platform>,
    #[prost(message, repeated, tag = "4")]
    pub decode_caps: Vec<CodecCap>,
    #[prost(message, optional, tag = "5")]
    pub display_prefs: Option<DisplayPrefs>,
    #[prost(enumeration = "Feature", repeated, tag = "6")]
    pub features: Vec<i32>,
    /// Relógio do controlador (ns), ecoado no `HelloAck`.
    #[prost(uint64, tag = "7")]
    pub clock_ns: u64,
}

#[derive(Clone, PartialEq, Message)]
pub struct Platform {
    /// "windows", "linux" (como o `OS` do Tailscale).
    #[prost(string, tag = "1")]
    pub os: String,
    /// "x86_64", "aarch64".
    #[prost(string, tag = "2")]
    pub arch: String,
    #[prost(string, tag = "3")]
    pub device_model: String,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct CodecCap {
    #[prost(enumeration = "Codec", tag = "1")]
    pub codec: i32,
    #[prost(uint32, tag = "2")]
    pub profile: u32,
    #[prost(uint32, tag = "3")]
    pub max_width: u32,
    #[prost(uint32, tag = "4")]
    pub max_height: u32,
    #[prost(bool, tag = "5")]
    pub chroma444: bool,
    #[prost(bool, tag = "6")]
    pub hw: bool,
    #[prost(bool, tag = "7")]
    pub gpu_output: bool,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct DisplayPrefs {
    #[prost(enumeration = "Fit", tag = "1")]
    pub fit: i32,
    #[prost(float, tag = "2")]
    pub scale_factor: f32,
    #[prost(uint32, tag = "3")]
    pub max_fps: u32,
}

/// H→C: resposta ao [`Hello`], com os monitores do host.
#[derive(Clone, PartialEq, Message)]
pub struct HelloAck {
    #[prost(uint32, tag = "1")]
    pub proto_version: u32,
    #[prost(string, tag = "2")]
    pub host_name: String,
    #[prost(message, optional, tag = "3")]
    pub platform: Option<Platform>,
    #[prost(message, repeated, tag = "4")]
    pub displays: Vec<DisplayInfo>,
    #[prost(message, repeated, tag = "5")]
    pub encode_caps: Vec<CodecCap>,
    #[prost(message, optional, tag = "6")]
    pub permissions: Option<Permissions>,
    #[prost(enumeration = "Consent", tag = "7")]
    pub consent: i32,
    /// Relógio do host (ns).
    #[prost(uint64, tag = "8")]
    pub clock_ns: u64,
    /// `Hello.clock_ns` ecoado, para estimar offset e RTT.
    #[prost(uint64, tag = "9")]
    pub client_clock_ns: u64,
}

/// O que o controlador pode fazer nesta sessão.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct Permissions {
    #[prost(bool, tag = "1")]
    pub view: bool,
    #[prost(bool, tag = "2")]
    pub control: bool,
    #[prost(bool, tag = "3")]
    pub clipboard: bool,
    #[prost(bool, tag = "4")]
    pub files: bool,
    /// Sem isso o host não captura nem envia áudio (`15-audio.md`).
    #[prost(bool, tag = "5")]
    pub audio: bool,
}

/// H→C: decisão da pessoa no host, quando o `HelloAck` pediu consentimento.
#[derive(Clone, PartialEq, Message)]
pub struct ConsentResult {
    #[prost(bool, tag = "1")]
    pub granted: bool,
    #[prost(message, optional, tag = "2")]
    pub permissions: Option<Permissions>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Close {
    #[prost(enumeration = "CloseReason", tag = "1")]
    pub reason: i32,
    #[prost(string, tag = "2")]
    pub message: String,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Ping {
    #[prost(uint64, tag = "1")]
    pub t_ns: u64,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct Pong {
    #[prost(uint64, tag = "1")]
    pub t_ns: u64,
    #[prost(uint64, tag = "2")]
    pub t_echo_ns: u64,
}

/// Recusa de um pedido; `display_id` diz qual monitor, quando for de vídeo.
#[derive(Clone, PartialEq, Message)]
pub struct Error {
    #[prost(enumeration = "ErrorCode", tag = "1")]
    pub code: i32,
    #[prost(uint32, optional, tag = "2")]
    pub display_id: Option<u32>,
    #[prost(string, tag = "3")]
    pub message: String,
}

// ---------------------------------------------------------------------------------------------
// Vídeo por monitor

/// Um monitor do host (`13-multimonitor.md` §3).
#[derive(Clone, PartialEq, Message)]
pub struct DisplayInfo {
    /// Estável na sessão; não reutilizar depois de removido.
    #[prost(uint32, tag = "1")]
    pub id: u32,
    /// Nome do EDID ou do portal; vazio se desconhecido.
    #[prost(string, tag = "2")]
    pub name: String,
    /// Posição no layout do host, em pixels lógicos (pode ser negativa).
    #[prost(int32, tag = "3")]
    pub x: i32,
    #[prost(int32, tag = "4")]
    pub y: i32,
    /// Pixels físicos capturados.
    #[prost(uint32, tag = "5")]
    pub width: u32,
    #[prost(uint32, tag = "6")]
    pub height: u32,
    #[prost(uint32, tag = "7")]
    pub logical_width: u32,
    #[prost(uint32, tag = "8")]
    pub logical_height: u32,
    #[prost(float, tag = "9")]
    pub scale: f32,
    #[prost(uint32, tag = "10")]
    pub refresh_mhz: u32,
    #[prost(bool, tag = "11")]
    pub primary: bool,
    #[prost(enumeration = "Rotation", tag = "12")]
    pub rotation: i32,
    /// `false` = existe mas o host não liberou (Wayland: fora da seleção do portal).
    #[prost(bool, tag = "13")]
    pub shared: bool,
}

/// H→C: monitor plugado, desplugado ou com resolução/escala nova.
#[derive(Clone, PartialEq, Message)]
pub struct DisplaysChanged {
    #[prost(message, repeated, tag = "1")]
    pub displays: Vec<DisplayInfo>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct QualityPrefs {
    #[prost(uint32, tag = "1")]
    pub fps_max: u32,
    #[prost(enumeration = "QualityMode", tag = "2")]
    pub mode: i32,
    #[prost(bool, tag = "3")]
    pub chroma444: bool,
}

/// C→H: começa o vídeo de um monitor. Vários podem estar ativos; trocar de monitor é
/// `StopVideo` + `StartVideo`.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct StartVideo {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    #[prost(message, optional, tag = "2")]
    pub wanted: Option<QualityPrefs>,
    #[prost(enumeration = "Priority", tag = "3")]
    pub priority: i32,
}

/// Espaço de cor em códigos da ITU-T H.273.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct ColorInfo {
    #[prost(uint32, tag = "1")]
    pub primaries: u32,
    #[prost(uint32, tag = "2")]
    pub transfer: u32,
    #[prost(uint32, tag = "3")]
    pub matrix: u32,
    #[prost(bool, tag = "4")]
    pub full_range: bool,
}

/// H→C: resposta a um `StartVideo`; os datagramas desse monitor vêm com `stream_id`.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct CodecSelected {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    /// `stream_id` do header de mídia, abaixo de [`AUDIO_STREAM_MIN`].
    #[prost(uint32, tag = "2")]
    pub stream_id: u32,
    #[prost(enumeration = "Codec", tag = "3")]
    pub codec: i32,
    #[prost(uint32, tag = "4")]
    pub profile: u32,
    #[prost(bool, tag = "5")]
    pub chroma444: bool,
    #[prost(uint32, tag = "6")]
    pub width: u32,
    #[prost(uint32, tag = "7")]
    pub height: u32,
    #[prost(uint32, tag = "8")]
    pub fps_target: u32,
    #[prost(uint32, tag = "9")]
    pub bitrate_kbps: u32,
    #[prost(message, optional, tag = "10")]
    pub color: Option<ColorInfo>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct StopVideo {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
}

/// Monitor em foco recebe qualidade cheia; os visíveis, menos fps. Escondido = `StopVideo`.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct SetPriority {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    #[prost(enumeration = "Priority", tag = "2")]
    pub priority: i32,
}

/// H→C: o vídeo de um monitor parou sem o controlador pedir (ou confirmando o `StopVideo`).
#[derive(Clone, Copy, PartialEq, Message)]
pub struct VideoStopped {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    #[prost(enumeration = "VideoStopReason", tag = "2")]
    pub reason: i32,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct RequestKeyframe {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct PictureLoss {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    #[prost(uint32, tag = "2")]
    pub last_good_seq: u32,
}

/// Fragmentos faltando de um frame: bit `i` do bitmap (LSB primeiro) = `frag_idx` `i`.
#[derive(Clone, PartialEq, Message)]
pub struct Nack {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    #[prost(uint32, tag = "2")]
    pub frame_seq: u32,
    #[prost(bytes = "vec", tag = "3")]
    pub missing_frag_bitmap: Vec<u8>,
}

// ---------------------------------------------------------------------------------------------
// Mouse e cursor

/// C→H: posição do mouse. Absoluta é normalizada dentro de um monitor; relativa é para jogos e
/// captura de ponteiro.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct MouseMove {
    #[prost(oneof = "mouse_move::Pos", tags = "1, 2")]
    pub pos: Option<mouse_move::Pos>,
}

pub mod mouse_move {
    #[derive(Clone, Copy, PartialEq, prost::Oneof)]
    pub enum Pos {
        #[prost(message, tag = "1")]
        Abs(super::AbsPos),
        #[prost(message, tag = "2")]
        Rel(super::RelPos),
    }
}

/// `x`, `y` em 0..=65535 dentro do monitor `display_id` (0 = borda esquerda/topo,
/// 65535 = direita/baixo).
#[derive(Clone, Copy, PartialEq, Message)]
pub struct AbsPos {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    #[prost(uint32, tag = "2")]
    pub x: u32,
    #[prost(uint32, tag = "3")]
    pub y: u32,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct RelPos {
    #[prost(sint32, tag = "1")]
    pub dx: i32,
    #[prost(sint32, tag = "2")]
    pub dy: i32,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct MouseButton {
    #[prost(enumeration = "Button", tag = "1")]
    pub button: i32,
    #[prost(bool, tag = "2")]
    pub down: bool,
}

/// Rolagem em "linhas" (`precise = false`, roda com dentes) ou em pixels lógicos (touchpad).
#[derive(Clone, Copy, PartialEq, Message)]
pub struct Scroll {
    #[prost(sint32, tag = "1")]
    pub dx: i32,
    #[prost(sint32, tag = "2")]
    pub dy: i32,
    #[prost(bool, tag = "3")]
    pub precise: bool,
}

/// H→C: posição do cursor do host, normalizada como em [`AbsPos`].
#[derive(Clone, Copy, PartialEq, Message)]
pub struct CursorPos {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    #[prost(uint32, tag = "2")]
    pub x: u32,
    #[prost(uint32, tag = "3")]
    pub y: u32,
    #[prost(bool, tag = "4")]
    pub visible: bool,
}

/// H→C: forma do cursor; o bitmap vai no stream de cursor, indexado pelo hash.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct CursorShapeRef {
    #[prost(uint64, tag = "1")]
    pub hash: u64,
}

// ---------------------------------------------------------------------------------------------
// Áudio (reservado)

#[derive(Clone, Copy, PartialEq, Message)]
pub struct AudioPrefs {
    #[prost(enumeration = "AudioProfile", tag = "1")]
    pub profile: i32,
    #[prost(uint32, tag = "2")]
    pub max_kbps: u32,
    #[prost(uint32, tag = "3")]
    pub channels: u32,
}

/// C→H: liga o áudio; `stream_id` em `AUDIO_STREAM_MIN..=0xFF`.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct StartAudio {
    #[prost(uint32, tag = "1")]
    pub stream_id: u32,
    #[prost(message, optional, tag = "2")]
    pub prefs: Option<AudioPrefs>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct AudioSelected {
    #[prost(uint32, tag = "1")]
    pub stream_id: u32,
    #[prost(enumeration = "AudioCodec", tag = "2")]
    pub codec: i32,
    #[prost(uint32, tag = "3")]
    pub sample_rate: u32,
    #[prost(uint32, tag = "4")]
    pub channels: u32,
    /// Duração do quadro em décimos de ms (25 = 2,5 ms; 200 = 20 ms).
    #[prost(uint32, tag = "5")]
    pub frame_ms_x10: u32,
    #[prost(uint32, tag = "6")]
    pub bitrate_kbps: u32,
    #[prost(bool, tag = "7")]
    pub redundancy: bool,
    #[prost(bool, tag = "8")]
    pub inband_fec: bool,
}

/// H→C: troca a quente de bitrate, quadro ou redundância.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct AudioConfig {
    #[prost(uint32, tag = "1")]
    pub stream_id: u32,
    #[prost(uint32, tag = "2")]
    pub bitrate_kbps: u32,
    #[prost(uint32, tag = "3")]
    pub frame_ms_x10: u32,
    #[prost(bool, tag = "4")]
    pub redundancy: bool,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct StopAudio {
    #[prost(uint32, tag = "1")]
    pub stream_id: u32,
}

/// H→C: o host já parou de enviar `kind=1` desse `stream_id` antes de mandar isto.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct AudioStopped {
    #[prost(uint32, tag = "1")]
    pub stream_id: u32,
    #[prost(enumeration = "AudioStopReason", tag = "2")]
    pub reason: i32,
}

// ---------------------------------------------------------------------------------------------
// Enums (0 = não especificado; valor desconhecido = tratar como 0)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum Feature {
    Unspecified = 0,
    Clipboard = 1,
    Files = 2,
    Audio = 3,
    MultiDisplay = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum Codec {
    Unspecified = 0,
    H264 = 1,
    Hevc = 2,
    Av1 = 3,
    Vp8 = 4,
    Vp9 = 5,
    PyroWave = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum Fit {
    Unspecified = 0,
    /// Escala para caber na janela.
    Window = 1,
    /// 1:1, com rolagem.
    Original = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum Consent {
    Unspecified = 0,
    Required = 1,
    NotRequired = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum CloseReason {
    Unspecified = 0,
    ByController = 1,
    ByHost = 2,
    ConsentDenied = 3,
    VersionMismatch = 4,
    ProtocolError = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum ErrorCode {
    Unspecified = 0,
    /// `StartVideo` acima do limite de streams simultâneos do host (2 com openh264).
    TooManyStreams = 1,
    UnknownDisplay = 2,
    /// Monitor existe mas não foi liberado (`DisplayInfo.shared = false`).
    DisplayNotShared = 3,
    NoPermission = 4,
    Unsupported = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum Rotation {
    Normal = 0,
    Rotate90 = 1,
    Rotate180 = 2,
    Rotate270 = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum QualityMode {
    Auto = 0,
    Text = 1,
    Motion = 2,
    LowBandwidth = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum Priority {
    Unspecified = 0,
    Focused = 1,
    Visible = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum VideoStopReason {
    Unspecified = 0,
    ByController = 1,
    DisplayRemoved = 2,
    /// Wayland: o compositor ou a pessoa fechou o stream do portal.
    PortalClosed = 3,
    NoPermission = 4,
    EncoderError = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum Button {
    Unspecified = 0,
    Left = 1,
    Right = 2,
    Middle = 3,
    Back = 4,
    Forward = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum AudioProfile {
    Auto = 0,
    Voice = 1,
    Music = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum AudioCodec {
    Unspecified = 0,
    Opus = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum AudioStopReason {
    Unspecified = 0,
    ByController = 1,
    ByHost = 2,
    NoPermission = 3,
    DeviceError = 4,
}

// ---------------------------------------------------------------------------------------------
// Limites e enquadramento

/// Mensagem que decodificou mas viola um limite do protocolo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidMessage {
    pub field: &'static str,
    pub value: u64,
    pub max: u64,
}

impl fmt::Display for InvalidMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "campo {} = {} fora do limite {}",
            self.field, self.value, self.max
        )
    }
}

impl std::error::Error for InvalidMessage {}

fn at_most(field: &'static str, value: usize, max: usize) -> Result<(), InvalidMessage> {
    if value > max {
        Err(InvalidMessage {
            field,
            value: value as u64,
            max: max as u64,
        })
    } else {
        Ok(())
    }
}

fn coord(field: &'static str, value: u32) -> Result<(), InvalidMessage> {
    at_most(field, value as usize, limits::COORD_MAX as usize)
}

fn video_stream(stream_id: u32) -> Result<(), InvalidMessage> {
    at_most(
        "stream_id",
        stream_id as usize,
        AUDIO_STREAM_MIN as usize - 1,
    )
}

fn audio_stream(stream_id: u32) -> Result<(), InvalidMessage> {
    if (u32::from(AUDIO_STREAM_MIN)..=u32::from(u8::MAX)).contains(&stream_id) {
        Ok(())
    } else {
        Err(InvalidMessage {
            field: "stream_id",
            value: stream_id.into(),
            max: u8::MAX.into(),
        })
    }
}

fn platform(p: Option<&Platform>) -> Result<(), InvalidMessage> {
    if let Some(p) = p {
        at_most("platform.os", p.os.len(), limits::SHORT_TEXT_LEN)?;
        at_most("platform.arch", p.arch.len(), limits::SHORT_TEXT_LEN)?;
        at_most(
            "platform.device_model",
            p.device_model.len(),
            limits::SHORT_TEXT_LEN,
        )?;
    }
    Ok(())
}

fn displays(list: &[DisplayInfo]) -> Result<(), InvalidMessage> {
    at_most("displays", list.len(), limits::DISPLAYS)?;
    for d in list {
        at_most("display.name", d.name.len(), limits::DISPLAY_NAME_LEN)?;
    }
    Ok(())
}

fn audio_prefs(p: Option<&AudioPrefs>) -> Result<(), InvalidMessage> {
    match p {
        Some(p) => at_most(
            "channels",
            p.channels as usize,
            limits::AUDIO_CHANNELS as usize,
        ),
        None => Ok(()),
    }
}

impl Control {
    /// Confere os limites do protocolo. [`decode_frame`] já chama.
    pub fn validate(&self) -> Result<(), InvalidMessage> {
        let Some(msg) = &self.msg else {
            return Ok(());
        };
        match msg {
            Msg::Hello(m) => {
                at_most("app_version", m.app_version.len(), limits::SHORT_TEXT_LEN)?;
                platform(m.platform.as_ref())?;
                at_most("decode_caps", m.decode_caps.len(), limits::CODEC_CAPS)?;
                at_most("features", m.features.len(), limits::FEATURES)
            }
            Msg::HelloAck(m) => {
                at_most("host_name", m.host_name.len(), limits::SHORT_TEXT_LEN)?;
                platform(m.platform.as_ref())?;
                displays(&m.displays)?;
                at_most("encode_caps", m.encode_caps.len(), limits::CODEC_CAPS)
            }
            Msg::Close(m) => at_most("message", m.message.len(), limits::MESSAGE_LEN),
            Msg::Error(m) => at_most("message", m.message.len(), limits::MESSAGE_LEN),
            Msg::DisplaysChanged(m) => displays(&m.displays),
            Msg::CodecSelected(m) => video_stream(m.stream_id),
            Msg::Nack(m) => at_most(
                "missing_frag_bitmap",
                m.missing_frag_bitmap.len(),
                limits::NACK_BITMAP_LEN,
            ),
            Msg::MouseMove(MouseMove {
                pos: Some(mouse_move::Pos::Abs(p)),
            }) => {
                coord("x", p.x)?;
                coord("y", p.y)
            }
            Msg::CursorPos(m) => {
                coord("x", m.x)?;
                coord("y", m.y)
            }
            Msg::StartAudio(m) => {
                audio_stream(m.stream_id)?;
                audio_prefs(m.prefs.as_ref())
            }
            Msg::AudioSelected(m) => {
                audio_stream(m.stream_id)?;
                at_most(
                    "channels",
                    m.channels as usize,
                    limits::AUDIO_CHANNELS as usize,
                )
            }
            Msg::AudioConfig(AudioConfig { stream_id, .. })
            | Msg::StopAudio(StopAudio { stream_id })
            | Msg::AudioStopped(AudioStopped { stream_id, .. }) => audio_stream(*stream_id),
            Msg::ConsentResult(_)
            | Msg::Ping(_)
            | Msg::Pong(_)
            | Msg::StartVideo(_)
            | Msg::StopVideo(_)
            | Msg::SetPriority(_)
            | Msg::VideoStopped(_)
            | Msg::RequestKeyframe(_)
            | Msg::PictureLoss(_)
            | Msg::MouseMove(_)
            | Msg::MouseButton(_)
            | Msg::Scroll(_)
            | Msg::CursorShapeRef(_) => Ok(()),
        }
    }
}

/// Erro ao ler um frame do stream de controle.
#[derive(Debug)]
pub enum FrameError {
    /// Prefixo de tamanho acima de [`limits::FRAME_LEN`]; a conexão deve ser fechada.
    TooLong(u64),
    /// Prefixo de tamanho malformado (varint com mais de 10 bytes).
    BadLength,
    Decode(prost::DecodeError),
    Invalid(InvalidMessage),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::TooLong(n) => write!(
                f,
                "mensagem de controle com {n} bytes, máximo {}",
                limits::FRAME_LEN
            ),
            FrameError::BadLength => f.write_str("prefixo de tamanho malformado"),
            FrameError::Decode(e) => write!(f, "mensagem de controle inválida: {e}"),
            FrameError::Invalid(e) => write!(f, "mensagem de controle inválida: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Acrescenta `msg` a `out`, precedida do tamanho em varint.
///
/// # Panics
/// Se a mensagem codificada passar de [`limits::FRAME_LEN`] (bug de quem envia: os limites de
/// [`Control::validate`] mantêm toda mensagem válida bem abaixo disso).
pub fn encode_frame(msg: &Control, out: &mut Vec<u8>) {
    let len = msg.encoded_len();
    assert!(
        len <= limits::FRAME_LEN,
        "mensagem de controle com {len} bytes"
    );
    msg.encode_length_delimited(out)
        .expect("Vec<u8> cresce o quanto precisar");
}

/// Lê um frame do início de `buf`.
///
/// `Ok(None)` = faltam bytes (leia mais e chame de novo); `Ok(Some((msg, n)))` = mensagem válida
/// que ocupou os `n` primeiros bytes. O tamanho é conferido antes de qualquer alocação.
pub fn decode_frame(buf: &[u8]) -> Result<Option<(Control, usize)>, FrameError> {
    let mut len: u64 = 0;
    let mut prefix = 0;
    loop {
        let Some(&b) = buf.get(prefix) else {
            return Ok(None);
        };
        if prefix == 10 {
            return Err(FrameError::BadLength);
        }
        len |= u64::from(b & 0x7f) << (7 * prefix);
        prefix += 1;
        if b & 0x80 == 0 {
            break;
        }
        if len > limits::FRAME_LEN as u64 {
            return Err(FrameError::TooLong(len));
        }
    }
    if len > limits::FRAME_LEN as u64 {
        return Err(FrameError::TooLong(len));
    }
    let end = prefix + len as usize;
    let Some(body) = buf.get(prefix..end) else {
        return Ok(None);
    };
    let msg = Control::decode(body).map_err(FrameError::Decode)?;
    msg.validate().map_err(FrameError::Invalid)?;
    Ok(Some((msg, end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(id: u32, name: &str, x: i32) -> DisplayInfo {
        DisplayInfo {
            id,
            name: name.into(),
            x,
            y: 0,
            width: 2560,
            height: 1440,
            logical_width: 2048,
            logical_height: 1152,
            scale: 1.25,
            refresh_mhz: 144_000,
            primary: id == 0,
            rotation: Rotation::Normal.into(),
            shared: true,
        }
    }

    fn todas() -> Vec<Msg> {
        let platform = Some(Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
            device_model: String::new(),
        });
        let cap = CodecCap {
            codec: Codec::H264.into(),
            profile: 66,
            max_width: 3840,
            max_height: 2160,
            chroma444: false,
            hw: false,
            gpu_output: false,
        };
        let perms = Permissions {
            view: true,
            control: true,
            clipboard: false,
            files: true,
            audio: false,
        };
        vec![
            Msg::Hello(Hello {
                proto_version: PROTO_VERSION,
                app_version: "0.2.0".into(),
                platform: platform.clone(),
                decode_caps: vec![cap],
                display_prefs: Some(DisplayPrefs {
                    fit: Fit::Window.into(),
                    scale_factor: 1.5,
                    max_fps: 60,
                }),
                features: vec![Feature::Files.into(), Feature::MultiDisplay.into()],
                clock_ns: 1_234_567_890,
            }),
            Msg::HelloAck(HelloAck {
                proto_version: PROTO_VERSION,
                host_name: "yurisp".into(),
                platform,
                displays: vec![monitor(0, "DELL U2720Q", 0), monitor(1, "", -1920)],
                encode_caps: vec![cap],
                permissions: Some(perms),
                consent: Consent::Required.into(),
                clock_ns: 99,
                client_clock_ns: 1_234_567_890,
            }),
            Msg::ConsentResult(ConsentResult {
                granted: true,
                permissions: Some(perms),
            }),
            Msg::Close(Close {
                reason: CloseReason::ByHost.into(),
                message: "tchau".into(),
            }),
            Msg::Ping(Ping { t_ns: 5 }),
            Msg::Pong(Pong {
                t_ns: 6,
                t_echo_ns: 5,
            }),
            Msg::Error(Error {
                code: ErrorCode::TooManyStreams.into(),
                display_id: Some(2),
                message: String::new(),
            }),
            Msg::StartVideo(StartVideo {
                display_id: 1,
                wanted: Some(QualityPrefs {
                    fps_max: 30,
                    mode: QualityMode::Text.into(),
                    chroma444: false,
                }),
                priority: Priority::Focused.into(),
            }),
            Msg::CodecSelected(CodecSelected {
                display_id: 1,
                stream_id: 1,
                codec: Codec::H264.into(),
                profile: 66,
                chroma444: false,
                width: 1920,
                height: 1080,
                fps_target: 30,
                bitrate_kbps: 8000,
                color: Some(ColorInfo {
                    primaries: 1,
                    transfer: 1,
                    matrix: 1,
                    full_range: false,
                }),
            }),
            Msg::StopVideo(StopVideo { display_id: 1 }),
            Msg::SetPriority(SetPriority {
                display_id: 0,
                priority: Priority::Visible.into(),
            }),
            Msg::VideoStopped(VideoStopped {
                display_id: 1,
                reason: VideoStopReason::PortalClosed.into(),
            }),
            Msg::DisplaysChanged(DisplaysChanged {
                displays: vec![monitor(0, "eDP-1", 0)],
            }),
            Msg::RequestKeyframe(RequestKeyframe { display_id: 0 }),
            Msg::PictureLoss(PictureLoss {
                display_id: 0,
                last_good_seq: 65_535,
            }),
            Msg::Nack(Nack {
                display_id: 0,
                frame_seq: 12,
                missing_frag_bitmap: vec![0b1010_0000, 1],
            }),
            Msg::MouseMove(MouseMove {
                pos: Some(mouse_move::Pos::Abs(AbsPos {
                    display_id: 1,
                    x: 65_535,
                    y: 0,
                })),
            }),
            Msg::MouseMove(MouseMove {
                pos: Some(mouse_move::Pos::Rel(RelPos { dx: -3, dy: 7 })),
            }),
            Msg::MouseButton(MouseButton {
                button: Button::Right.into(),
                down: true,
            }),
            Msg::Scroll(Scroll {
                dx: 0,
                dy: -120,
                precise: true,
            }),
            Msg::CursorPos(CursorPos {
                display_id: 1,
                x: 100,
                y: 200,
                visible: true,
            }),
            Msg::CursorShapeRef(CursorShapeRef { hash: u64::MAX }),
            Msg::StartAudio(StartAudio {
                stream_id: 0xF0,
                prefs: Some(AudioPrefs {
                    profile: AudioProfile::Music.into(),
                    max_kbps: 128,
                    channels: 2,
                }),
            }),
            Msg::AudioSelected(AudioSelected {
                stream_id: 0xF0,
                codec: AudioCodec::Opus.into(),
                sample_rate: 48_000,
                channels: 2,
                frame_ms_x10: 100,
                bitrate_kbps: 96,
                redundancy: true,
                inband_fec: false,
            }),
            Msg::AudioConfig(AudioConfig {
                stream_id: 0xF0,
                bitrate_kbps: 32,
                frame_ms_x10: 200,
                redundancy: false,
            }),
            Msg::StopAudio(StopAudio { stream_id: 0xFF }),
            Msg::AudioStopped(AudioStopped {
                stream_id: 0xF0,
                reason: AudioStopReason::ByController.into(),
            }),
        ]
    }

    #[test]
    fn round_trip_de_todas_as_mensagens() {
        let msgs = todas();
        let mut fio = Vec::new();
        for m in &msgs {
            encode_frame(&m.clone().into(), &mut fio);
        }
        let mut resto = fio.as_slice();
        for esperado in msgs {
            let (lida, n) = decode_frame(resto).unwrap().unwrap();
            assert_eq!(lida.msg, Some(esperado));
            resto = &resto[n..];
        }
        assert!(resto.is_empty());
    }

    #[test]
    fn frame_incompleto_pede_mais_bytes() {
        let mut fio = Vec::new();
        encode_frame(&Msg::Ping(Ping { t_ns: 300 }).into(), &mut fio);
        for corte in 0..fio.len() {
            assert!(decode_frame(&fio[..corte]).unwrap().is_none(), "{corte}");
        }
        assert!(decode_frame(&fio).unwrap().is_some());
    }

    #[test]
    fn tamanho_acima_do_limite_falha_antes_de_ler() {
        // varint de FRAME_LEN + 1 sem nenhum byte de corpo.
        let mut fio = Vec::new();
        prost::encoding::encode_varint(limits::FRAME_LEN as u64 + 1, &mut fio);
        assert!(matches!(decode_frame(&fio), Err(FrameError::TooLong(_))));
        let enorme = [0xff; 11];
        assert!(decode_frame(&enorme).is_err());
    }

    #[test]
    fn tag_desconhecida_vira_none() {
        // Campo 55 (faixa do teclado, ainda sem mensagem), tipo length-delimited, vazio.
        let corpo = [(55 << 3 | 2) as u8 | 0x80, 55 >> 4, 0];
        let mut fio = vec![corpo.len() as u8];
        fio.extend_from_slice(&corpo);
        let (msg, n) = decode_frame(&fio).unwrap().unwrap();
        assert_eq!(msg.msg, None);
        assert_eq!(n, fio.len());
    }

    fn invalida(msg: Msg) -> InvalidMessage {
        let mut fio = Vec::new();
        Control::from(msg)
            .encode_length_delimited(&mut fio)
            .unwrap();
        match decode_frame(&fio) {
            Err(FrameError::Invalid(e)) => e,
            outro => panic!("esperava InvalidMessage, veio {outro:?}"),
        }
    }

    #[test]
    fn limites_de_monitores() {
        let muitos = (0..17).map(|i| monitor(i, "", 0)).collect();
        let e = invalida(Msg::DisplaysChanged(DisplaysChanged { displays: muitos }));
        assert_eq!(e.field, "displays");
        let nome = "x".repeat(limits::DISPLAY_NAME_LEN + 1);
        let e = invalida(Msg::DisplaysChanged(DisplaysChanged {
            displays: vec![monitor(0, &nome, 0)],
        }));
        assert_eq!(e.field, "display.name");
        let dezesseis = (0..16).map(|i| monitor(i, "", 0)).collect();
        assert!(
            Control::from(Msg::DisplaysChanged(DisplaysChanged {
                displays: dezesseis
            }))
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn stream_ids_separam_video_e_audio() {
        let video = |stream_id| {
            Msg::CodecSelected(CodecSelected {
                display_id: 0,
                stream_id,
                codec: Codec::H264.into(),
                profile: 66,
                chroma444: false,
                width: 1,
                height: 1,
                fps_target: 30,
                bitrate_kbps: 1,
                color: None,
            })
        };
        assert!(Control::from(video(0xEF)).validate().is_ok());
        assert_eq!(invalida(video(0xF0)).field, "stream_id");
        assert_eq!(
            invalida(Msg::StopAudio(StopAudio { stream_id: 0xEF })).field,
            "stream_id"
        );
        assert_eq!(
            invalida(Msg::StopAudio(StopAudio { stream_id: 0x100 })).field,
            "stream_id"
        );
    }

    #[test]
    fn coordenadas_normalizadas_ate_65535() {
        let e = invalida(Msg::MouseMove(MouseMove {
            pos: Some(mouse_move::Pos::Abs(AbsPos {
                display_id: 0,
                x: 65_536,
                y: 0,
            })),
        }));
        assert_eq!(e.field, "x");
        let e = invalida(Msg::CursorPos(CursorPos {
            display_id: 0,
            x: 0,
            y: 70_000,
            visible: true,
        }));
        assert_eq!(e.field, "y");
    }

    #[test]
    fn enum_desconhecido_vira_padrao() {
        let msg = SetPriority {
            display_id: 0,
            priority: 42,
        };
        assert_eq!(msg.priority(), Priority::Unspecified);
    }
}
