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
//! | 70–79    | Área de transferência (fase 4)                           |
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

use crate::files::limits as files;
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
    /// Nome de um backend de codec (`VideoStats.encoder`).
    pub const CODEC_NAME_LEN: usize = 32;
    /// Recursos no `Hello`.
    pub const FEATURES: usize = 32;
    /// Bitmap de fragmentos de um `Nack`: `frag_count` cabe em u16, então 8 KiB cobre todos.
    pub const NACK_BITMAP_LEN: usize = 8 * 1024;
    /// Coordenada normalizada dentro do monitor (`MouseMove`, `CursorPos`).
    pub const COORD_MAX: u32 = u16::MAX as u32;
    /// Canais de áudio.
    pub const AUDIO_CHANNELS: u32 = 2;
    /// Taxa fixa de vídeo pedida em `QualityPrefs` (1 Gbit/s).
    pub const VIDEO_KBPS: u32 = 1_000_000;
    /// Texto de um `KeyChar`, em bytes (uma tecla, ou a string de um IME).
    pub const KEY_TEXT_LEN: usize = 256;
    /// Teclas físicas extras em `KeyboardInfo.extra_hid`.
    pub const EXTRA_HID: usize = 32;
    /// Maior HID usage de teclado (página 0x07).
    pub const HID_MAX: u32 = 0xFFFF;
    /// Raízes liberadas pelo host para o navegador remoto.
    pub const FS_ROOTS: usize = 64;
    /// Nome de exibição de uma raiz.
    pub const FS_ROOT_NAME_LEN: usize = 128;
    /// Senha de acesso no `Hello`, em bytes.
    pub const PASSWORD_LEN: usize = 128;
    /// Formatos numa `ClipboardOffer`.
    pub const CLIP_FORMATS: usize = 8;
    /// Arquivos e pastas copiados listados numa `ClipboardOffer` (o resto não vai).
    pub const CLIP_FILES: usize = 256;
    /// Bytes de um pedaço de `ClipboardData` (cabe folgado num frame de controle).
    pub const CLIP_CHUNK: usize = 32 * 1024;
    /// Maior conteúdo de um formato (uma captura de tela 4K em PNG passa com folga).
    pub const CLIP_TOTAL: u64 = 64 * 1024 * 1024;
    // Caminhos, nomes e itens por página: `crate::files::limits`.
}

// ---------------------------------------------------------------------------------------------
// Envelope

/// Uma mensagem do stream de controle.
#[derive(Clone, PartialEq, Message)]
pub struct Control {
    #[prost(
        oneof = "Msg",
        tags = "1, 2, 3, 4, 5, 6, 7, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 40, 41, 42, 50, 51, 52, 53, 54, 55, 60, 61, 70, 71, 72, 73, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 100, 101, 102, 103, 104"
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
    #[prost(message, tag = "29")]
    VideoStats(VideoStats),
    #[prost(message, tag = "30")]
    SetVideoQuality(SetVideoQuality),

    // Mouse
    #[prost(message, tag = "40")]
    MouseMove(MouseMove),
    #[prost(message, tag = "41")]
    MouseButton(MouseButton),
    #[prost(message, tag = "42")]
    Scroll(Scroll),

    // Teclado (`11-teclado.md` §3)
    #[prost(message, tag = "50")]
    KeyboardLayoutChanged(KeyboardLayoutChanged),
    #[prost(message, tag = "51")]
    KeyboardMode(KeyboardMode),
    #[prost(message, tag = "52")]
    Key(Key),
    #[prost(message, tag = "53")]
    KeyShortcut(KeyShortcut),
    #[prost(message, tag = "54")]
    KeyChar(KeyChar),
    #[prost(message, tag = "55")]
    ReleaseAll(ReleaseAll),

    // Cursor
    #[prost(message, tag = "60")]
    CursorPos(CursorPos),
    #[prost(message, tag = "61")]
    CursorShapeRef(CursorShapeRef),

    // Área de transferência (fase 4): oferta com os formatos, conteúdo só sob pedido
    #[prost(message, tag = "70")]
    ClipboardOffer(ClipboardOffer),
    #[prost(message, tag = "71")]
    ClipboardRequest(ClipboardRequest),
    #[prost(message, tag = "72")]
    ClipboardData(ClipboardData),
    #[prost(message, tag = "73")]
    ClipboardError(ClipboardError),

    // Arquivos (`10-plano-windows-linux.md` §3; bytes nos streams uni, `crate::files`)
    #[prost(message, tag = "80")]
    FsRootsRequest(FsRootsRequest),
    #[prost(message, tag = "81")]
    FsRoots(FsRoots),
    #[prost(message, tag = "82")]
    FsList(FsList),
    #[prost(message, tag = "83")]
    FsListing(FsListing),
    #[prost(message, tag = "84")]
    FileOffer(FileOffer),
    #[prost(message, tag = "85")]
    FileRequest(FileRequest),
    #[prost(message, tag = "86")]
    FileAccept(FileAccept),
    #[prost(message, tag = "87")]
    FileCancel(FileCancel),
    #[prost(message, tag = "88")]
    FileDone(FileDone),
    #[prost(message, tag = "89")]
    FileError(FileError),

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
/// O `Debug` esconde a senha.
#[derive(Clone, PartialEq, Message)]
#[prost(skip_debug)]
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
    /// Layout do teclado do controlador.
    #[prost(message, optional, tag = "8")]
    pub keyboard: Option<KeyboardInfo>,
    /// Senha de acesso deste host (salva no controlador): certa, entra sem o diálogo; errada,
    /// `Close(BAD_PASSWORD)`. Vazia = pedir acesso. Vai só dentro do QUIC com o certificado do
    /// host conferido; o host guarda só o hash (argon2).
    #[prost(string, tag = "9")]
    pub password: String,
}

impl std::fmt::Debug for Hello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hello")
            .field("proto_version", &self.proto_version)
            .field("app_version", &self.app_version)
            .field("platform", &self.platform)
            .field("decode_caps", &self.decode_caps)
            .field("display_prefs", &self.display_prefs)
            .field("features", &self.features)
            .field("clock_ns", &self.clock_ns)
            .field("keyboard", &self.keyboard)
            .field("password", &(!self.password.is_empty()).then_some("***"))
            .finish()
    }
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
    /// Layout do teclado do host.
    #[prost(message, optional, tag = "10")]
    pub keyboard: Option<KeyboardInfo>,
    /// O host aceita senha de acesso (o controlador pode oferecer o campo enquanto espera).
    #[prost(bool, tag = "11")]
    pub password_enabled: bool,
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
    /// Codec escolhido pelo usuário; `Unspecified` = automático. O host só atende se o
    /// controlador decodifica (`Hello.decode_caps`) e ele tem encoder; senão fica no automático.
    #[prost(enumeration = "Codec", tag = "4")]
    pub codec: i32,
    /// Taxa fixa; `0` = automática (o host ajusta pelo tamanho da imagem e pela rede).
    #[prost(uint32, tag = "5")]
    pub bitrate_kbps: u32,
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

/// C→H: troca codec ou taxa de um monitor que já está saindo (seletor da janela da sessão).
/// Codec novo = encoder novo e outro `CodecSelected` (com outro `stream_id`); só a taxa =
/// ajuste a quente, sem `CodecSelected`.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct SetVideoQuality {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    #[prost(message, optional, tag = "2")]
    pub wanted: Option<QualityPrefs>,
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

/// H→C: como o vídeo de um monitor está saindo do host, uma vez por segundo, para o painel de
/// diagnóstico. Só informativo: o controlador não decide nada por ela.
#[derive(Clone, PartialEq, Message)]
pub struct VideoStats {
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    /// Backend do encoder (`nvenc`, `vaapi`, `mf`, `openh264`).
    #[prost(string, tag = "2")]
    pub encoder: String,
    /// GPU que codifica; vazio = CPU.
    #[prost(string, tag = "3")]
    pub gpu: String,
    /// Quadros enviados por segundo, × 100.
    #[prost(uint32, tag = "4")]
    pub fps_x100: u32,
    /// Taxa que saiu no último intervalo.
    #[prost(uint32, tag = "5")]
    pub bitrate_kbps: u32,
    /// Alvo atual do controle de taxa.
    #[prost(uint32, tag = "6")]
    pub target_kbps: u32,
    /// Média de captura → access unit (conversão + encode), em µs.
    #[prost(uint32, tag = "7")]
    pub encode_us: u32,
    /// Quadros capturados que não saíram (substituídos ou pulados pelo controle de taxa).
    #[prost(uint32, tag = "8")]
    pub skipped: u32,
    #[prost(uint32, tag = "9")]
    pub keyframes: u32,
    /// Pacotes QUIC que o host mandou e deu como perdidos no intervalo, em partes por milhão.
    #[prost(uint32, tag = "10")]
    pub loss_ppm: u32,
    /// Tamanho real dos quadros que saem (o da captura, em pixels físicos).
    #[prost(uint32, tag = "11")]
    pub width: u32,
    #[prost(uint32, tag = "12")]
    pub height: u32,
    /// Banda que o QUIC do host estima para este caminho (janela de congestionamento ÷ RTT),
    /// em kbit/s; `0` = sem estimativa. Com o vídeo limitando o envio, fica perto do que sai.
    #[prost(uint32, tag = "13")]
    pub est_kbps: u32,
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
// Teclado (`11-teclado.md` §3)

/// Layout de um lado, no `Hello`/`HelloAck` e em [`KeyboardLayoutChanged`]. A tradução usa as
/// tabelas lidas do SO de cada lado; `layout_tag` só decide "iguais ou diferentes", UI e logs.
#[derive(Clone, PartialEq, Message)]
pub struct KeyboardInfo {
    /// KLID no Windows (`"00020409"`) ou `"xkb:us(intl)"` no Linux.
    #[prost(string, tag = "1")]
    pub os_layout_id: String,
    /// Normalizado: `"us-intl"`, `"br-abnt2"`, `"us"`...
    #[prost(string, tag = "2")]
    pub layout_tag: String,
    /// Teclas físicas que existem além do básico (0x87 e 0x85 no ABNT2).
    #[prost(uint32, repeated, tag = "3")]
    pub extra_hid: Vec<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct KeyboardLayoutChanged {
    #[prost(message, optional, tag = "1")]
    pub info: Option<KeyboardInfo>,
}

/// C→H: escolha do usuário para este host.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct KeyboardMode {
    #[prost(enumeration = "KeyboardModeKind", tag = "1")]
    pub mode: i32,
}

/// C→H: tecla física (HID usage, página 0x07).
#[derive(Clone, Copy, PartialEq, Message)]
pub struct Key {
    #[prost(uint32, tag = "1")]
    pub hid: u32,
    #[prost(bool, tag = "2")]
    pub down: bool,
    /// Só no modo Físico: bit 0 = Caps Lock, bit 1 = Num Lock do controlador.
    #[prost(uint32, optional, tag = "3")]
    pub locks: Option<u32>,
}

/// C→H: atalho pela letra. `base_char` é o que a tecla produz sem modificador no controlador.
#[derive(Clone, PartialEq, Message)]
pub struct KeyShortcut {
    #[prost(string, tag = "1")]
    pub base_char: String,
    #[prost(uint32, tag = "2")]
    pub hid_hint: u32,
    /// Bits: 0 Shift, 1 Ctrl, 2 Alt, 3 Meta.
    #[prost(uint32, tag = "3")]
    pub mods: u32,
    #[prost(bool, tag = "4")]
    pub down: bool,
}

/// C→H: texto já resolvido no controlador (teclas mortas compostas).
#[derive(Clone, PartialEq, Message)]
pub struct KeyChar {
    #[prost(string, tag = "1")]
    pub text: String,
    #[prost(uint32, tag = "2")]
    pub hid_hint: u32,
}

/// C→H: soltar tudo que estiver apertado (perda de foco).
#[derive(Clone, Copy, PartialEq, Message)]
pub struct ReleaseAll {}

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
// Área de transferência
//
// Quem copia manda uma `ClipboardOffer` com os formatos (e, para arquivos, a lista); o outro
// lado põe a oferta na área de transferência do sistema sem o conteúdo, e só quando alguém cola
// pede o formato (`ClipboardRequest`). O conteúdo volta em pedaços de até
// `limits::CLIP_CHUNK` (`ClipboardData`), entre as outras mensagens de controle, para um PNG
// grande não segurar o mouse. Cada oferta tem um `seq` próprio de quem oferece; pedido de uma
// oferta que já foi trocada recebe `ClipboardError(GONE)`. Sem `Permissions.clipboard` nada
// disso é atendido. Arquivos colados vêm pelos streams de arquivos (`FileRequest` na raiz
// `crate::files::CLIPBOARD_ROOT`).

/// A área de transferência de quem manda mudou (por alguém de lá, nunca pela oferta do peer).
/// `formats` vazio = ficou vazia.
#[derive(Clone, PartialEq, Message)]
pub struct ClipboardOffer {
    #[prost(uint32, tag = "1")]
    pub seq: u32,
    #[prost(enumeration = "ClipFormat", repeated, tag = "2")]
    pub formats: Vec<i32>,
    /// Tamanho de cada formato na ordem de `formats`, quando se sabe (0 = desconhecido).
    #[prost(uint64, repeated, tag = "3")]
    pub sizes: Vec<u64>,
    /// Com `ClipFormat::Files`: o que foi copiado (só o primeiro nível).
    #[prost(message, repeated, tag = "4")]
    pub files: Vec<ClipFile>,
}

/// Um arquivo ou pasta copiado. O índice na lista é o caminho dele na raiz da área de
/// transferência (`"<índice>/<nome>"`).
#[derive(Clone, PartialEq, Message)]
pub struct ClipFile {
    #[prost(string, tag = "1")]
    pub name: String,
    /// Bytes (de uma pasta: 0).
    #[prost(uint64, tag = "2")]
    pub size: u64,
    #[prost(bool, tag = "3")]
    pub dir: bool,
}

/// Alguém colou: manda o conteúdo de `format` da oferta `seq`.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct ClipboardRequest {
    #[prost(uint32, tag = "1")]
    pub seq: u32,
    #[prost(enumeration = "ClipFormat", tag = "2")]
    pub format: i32,
}

/// Um pedaço do conteúdo: `data` vai em `offset` de um total de `total` bytes; o último é o
/// que chega a `total`. Conteúdo vazio = um pedaço só com `total` = 0.
#[derive(Clone, PartialEq, Message)]
pub struct ClipboardData {
    #[prost(uint32, tag = "1")]
    pub seq: u32,
    #[prost(enumeration = "ClipFormat", tag = "2")]
    pub format: i32,
    #[prost(uint64, tag = "3")]
    pub offset: u64,
    #[prost(uint64, tag = "4")]
    pub total: u64,
    #[prost(bytes = "vec", tag = "5")]
    pub data: Vec<u8>,
}

/// O pedido não pode ser atendido.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct ClipboardError {
    #[prost(uint32, tag = "1")]
    pub seq: u32,
    #[prost(enumeration = "ClipFormat", tag = "2")]
    pub format: i32,
    #[prost(enumeration = "ClipErrorCode", tag = "3")]
    pub code: i32,
}

// ---------------------------------------------------------------------------------------------
// Arquivos
//
// Caminhos são relativos a uma raiz (`root`), com `/` como separador, e quem recebe sempre os
// passa por `taildesk_files::resolve_in_root`. Raiz 0 (`crate::files::INBOX_ROOT`) é a pasta de
// entrada de quem recebe. Fluxo: `FileOffer` (no download, depois do `FileRequest`) →
// `FileAccept` com offset e prefixo → stream uni com `crate::files::DataHeader` → `FileDone`.

/// C→H: pede as pastas que o host libera para o navegador remoto.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct FsRootsRequest {}

/// H→C: pastas liberadas (vazio = navegador remoto desligado).
#[derive(Clone, PartialEq, Message)]
pub struct FsRoots {
    #[prost(message, repeated, tag = "1")]
    pub roots: Vec<FsRoot>,
}

#[derive(Clone, PartialEq, Message)]
pub struct FsRoot {
    /// Diferente de 0, estável na sessão.
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(enumeration = "FsAccess", tag = "3")]
    pub access: i32,
}

/// Lista `path` (`""` = a própria raiz) a partir do item `start`.
#[derive(Clone, PartialEq, Message)]
pub struct FsList {
    #[prost(uint32, tag = "1")]
    pub req_id: u32,
    #[prost(uint32, tag = "2")]
    pub root: u32,
    #[prost(string, tag = "3")]
    pub path: String,
    #[prost(uint32, tag = "4")]
    pub start: u32,
}

/// Uma página da listagem; `more` = pedir de novo com `start` += `entries.len()`.
#[derive(Clone, PartialEq, Message)]
pub struct FsListing {
    #[prost(uint32, tag = "1")]
    pub req_id: u32,
    #[prost(message, repeated, tag = "2")]
    pub entries: Vec<FsEntry>,
    #[prost(bool, tag = "3")]
    pub more: bool,
    /// Preenchido quando a listagem falhou (`entries` vazio).
    #[prost(enumeration = "FileErrorCode", tag = "4")]
    pub error: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct FsEntry {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(enumeration = "FsEntryKind", tag = "2")]
    pub kind: i32,
    #[prost(uint64, tag = "3")]
    pub size: u64,
    /// Milissegundos desde 1970 (UTC); 0 se desconhecido.
    #[prost(int64, tag = "4")]
    pub mtime_ms: i64,
}

/// Quem envia oferece um arquivo para `rel_path` dentro de `root` de quem recebe.
#[derive(Clone, PartialEq, Message)]
pub struct FileOffer {
    #[prost(uint32, tag = "1")]
    pub xfer_id: u32,
    #[prost(uint32, tag = "2")]
    pub root: u32,
    #[prost(string, tag = "3")]
    pub rel_path: String,
    #[prost(uint64, tag = "4")]
    pub size: u64,
    #[prost(int64, tag = "5")]
    pub mtime_ms: i64,
}

/// Download: pede que o peer ofereça `path` de `root` com este `xfer_id`.
#[derive(Clone, PartialEq, Message)]
pub struct FileRequest {
    #[prost(uint32, tag = "1")]
    pub xfer_id: u32,
    #[prost(uint32, tag = "2")]
    pub root: u32,
    #[prost(string, tag = "3")]
    pub path: String,
}

/// Aceita a oferta a partir de `offset` (tamanho do `.part`); `prefix_blake3` é o BLAKE3
/// desses bytes, vazio quando `offset` = 0. Prefixo diferente = `FileError(RESUME_MISMATCH)`.
#[derive(Clone, PartialEq, Message)]
pub struct FileAccept {
    #[prost(uint32, tag = "1")]
    pub xfer_id: u32,
    #[prost(uint64, tag = "2")]
    pub offset: u64,
    #[prost(bytes = "vec", tag = "3")]
    pub prefix_blake3: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Message)]
pub struct FileCancel {
    #[prost(uint32, tag = "1")]
    pub xfer_id: u32,
}

/// Fim do envio, com o BLAKE3 do arquivo inteiro.
#[derive(Clone, PartialEq, Message)]
pub struct FileDone {
    #[prost(uint32, tag = "1")]
    pub xfer_id: u32,
    #[prost(bytes = "vec", tag = "2")]
    pub blake3: Vec<u8>,
}

/// Recusa ou falha de uma transferência, nos dois sentidos.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct FileError {
    #[prost(uint32, tag = "1")]
    pub xfer_id: u32,
    #[prost(enumeration = "FileErrorCode", tag = "2")]
    pub code: i32,
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
pub enum KeyboardModeKind {
    /// Layouts iguais: físico; diferentes: híbrido.
    Auto = 0,
    Physical = 1,
    Text = 2,
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
    /// Ninguém respondeu ao pedido de acesso no host (60 s, `12-levantamento-ux.md` §5.3).
    ConsentTimeout = 6,
    /// O host não aceita conexões deste peer (configuração ou política).
    NotAllowed = 7,
    /// Senha de acesso errada (ou o host não tem mais senha).
    BadPassword = 8,
    /// Senhas erradas demais deste peer: o host não confere por um tempo.
    TooManyAttempts = 9,
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

/// Formato da área de transferência.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum ClipFormat {
    Unspecified = 0,
    /// Texto em UTF-8, quebras de linha `\n`.
    Text = 1,
    /// Fragmento HTML em UTF-8 (sem o cabeçalho "HTML Format" do Windows).
    Html = 2,
    /// Imagem em PNG.
    Png = 3,
    /// Arquivos e pastas (`ClipboardOffer.files`); o conteúdo vem pelos streams de arquivos.
    Files = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum ClipErrorCode {
    Unspecified = 0,
    /// A oferta `seq` já foi trocada por outra (ou nunca existiu).
    Gone = 1,
    /// Sem `Permissions.clipboard`.
    Denied = 2,
    /// Passa de `limits::CLIP_TOTAL`.
    TooBig = 3,
    /// O sistema não entregou o formato (o app que copiou fechou, por exemplo).
    Unavailable = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum FsAccess {
    Unspecified = 0,
    /// Só listar e baixar.
    Read = 1,
    /// Só enviar para ela.
    Write = 2,
    ReadWrite = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum FsEntryKind {
    Unspecified = 0,
    Dir = 1,
    File = 2,
    /// Dispositivo, socket etc.: aparece, mas não pode ser baixado.
    Other = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum FileErrorCode {
    Unspecified = 0,
    NotFound = 1,
    /// Sem `Permissions.files`, raiz sem o acesso pedido ou recusado pela pessoa.
    Denied = 2,
    /// Caminho recusado por `resolve_in_root`.
    BadPath = 3,
    NoSpace = 4,
    /// O BLAKE3 do arquivo recebido não bate com o do `FileDone`.
    HashMismatch = 5,
    /// O `.part` não bate com o arquivo de quem envia: recomeçar do zero.
    ResumeMismatch = 6,
    Io = 7,
    /// Mais transferências simultâneas que `crate::files::limits::PARALLEL`.
    Busy = 8,
    Cancelled = 9,
    /// O arquivo de origem mudou durante o envio.
    Changed = 10,
    /// Stream de dados com cabeçalho errado ou bytes a mais.
    Protocol = 11,
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

fn at_most_u64(field: &'static str, value: u64, max: u64) -> Result<(), InvalidMessage> {
    if value > max {
        Err(InvalidMessage { field, value, max })
    } else {
        Ok(())
    }
}

fn exactly(field: &'static str, value: usize, want: usize) -> Result<(), InvalidMessage> {
    if value == want {
        Ok(())
    } else {
        Err(InvalidMessage {
            field,
            value: value as u64,
            max: want as u64,
        })
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

fn hid(field: &'static str, value: u32) -> Result<(), InvalidMessage> {
    at_most(field, value as usize, limits::HID_MAX as usize)
}

fn keyboard(k: Option<&KeyboardInfo>) -> Result<(), InvalidMessage> {
    if let Some(k) = k {
        at_most(
            "keyboard.os_layout_id",
            k.os_layout_id.len(),
            limits::SHORT_TEXT_LEN,
        )?;
        at_most(
            "keyboard.layout_tag",
            k.layout_tag.len(),
            limits::SHORT_TEXT_LEN,
        )?;
        at_most("keyboard.extra_hid", k.extra_hid.len(), limits::EXTRA_HID)?;
        for &h in &k.extra_hid {
            hid("keyboard.extra_hid", h)?;
        }
    }
    Ok(())
}

fn quality_prefs(p: Option<&QualityPrefs>) -> Result<(), InvalidMessage> {
    let Some(p) = p else { return Ok(()) };
    at_most(
        "bitrate_kbps",
        p.bitrate_kbps as usize,
        limits::VIDEO_KBPS as usize,
    )
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
            Msg::ClipboardOffer(m) => {
                at_most("formats", m.formats.len(), limits::CLIP_FORMATS)?;
                at_most("sizes", m.sizes.len(), limits::CLIP_FORMATS)?;
                at_most("files", m.files.len(), limits::CLIP_FILES)?;
                for f in &m.files {
                    at_most("file.name", f.name.len(), files::NAME_LEN)?;
                }
                Ok(())
            }
            Msg::ClipboardData(m) => {
                at_most("data", m.data.len(), limits::CLIP_CHUNK)?;
                at_most_u64("total", m.total, limits::CLIP_TOTAL)?;
                let fim = m.offset.saturating_add(m.data.len() as u64);
                at_most_u64("offset", fim, m.total)
            }
            Msg::FsRoots(m) => {
                at_most("roots", m.roots.len(), limits::FS_ROOTS)?;
                for r in &m.roots {
                    at_most("root.name", r.name.len(), limits::FS_ROOT_NAME_LEN)?;
                    if r.id == crate::files::INBOX_ROOT || r.id == crate::files::CLIPBOARD_ROOT {
                        return Err(InvalidMessage {
                            field: "root.id",
                            value: 0,
                            max: 0,
                        });
                    }
                }
                Ok(())
            }
            Msg::FsList(FsList { path, .. }) | Msg::FileRequest(FileRequest { path, .. }) => {
                at_most("path", path.len(), files::PATH_LEN)
            }
            Msg::FsListing(m) => {
                at_most("entries", m.entries.len(), files::LISTING_ENTRIES)?;
                for e in &m.entries {
                    at_most("entry.name", e.name.len(), files::NAME_LEN)?;
                }
                Ok(())
            }
            Msg::FileOffer(m) => at_most("rel_path", m.rel_path.len(), files::PATH_LEN),
            Msg::FileAccept(m) => {
                let want = if m.offset > 0 { 32 } else { 0 };
                exactly("prefix_blake3", m.prefix_blake3.len(), want)
            }
            Msg::FileDone(m) => exactly("blake3", m.blake3.len(), 32),
            Msg::Hello(m) => {
                at_most("app_version", m.app_version.len(), limits::SHORT_TEXT_LEN)?;
                at_most("password", m.password.len(), limits::PASSWORD_LEN)?;
                keyboard(m.keyboard.as_ref())?;
                platform(m.platform.as_ref())?;
                at_most("decode_caps", m.decode_caps.len(), limits::CODEC_CAPS)?;
                at_most("features", m.features.len(), limits::FEATURES)
            }
            Msg::HelloAck(m) => {
                at_most("host_name", m.host_name.len(), limits::SHORT_TEXT_LEN)?;
                keyboard(m.keyboard.as_ref())?;
                platform(m.platform.as_ref())?;
                displays(&m.displays)?;
                at_most("encode_caps", m.encode_caps.len(), limits::CODEC_CAPS)
            }
            Msg::Close(m) => at_most("message", m.message.len(), limits::MESSAGE_LEN),
            Msg::Error(m) => at_most("message", m.message.len(), limits::MESSAGE_LEN),
            Msg::DisplaysChanged(m) => displays(&m.displays),
            Msg::CodecSelected(m) => video_stream(m.stream_id),
            Msg::VideoStats(m) => {
                at_most("encoder", m.encoder.len(), limits::CODEC_NAME_LEN)?;
                at_most("gpu", m.gpu.len(), limits::SHORT_TEXT_LEN)
            }
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
            Msg::KeyboardLayoutChanged(m) => keyboard(m.info.as_ref()),
            Msg::Key(m) => hid("hid", m.hid),
            Msg::KeyShortcut(m) => {
                // Um caractere só (até 4 bytes em UTF-8).
                at_most("base_char", m.base_char.chars().count(), 1)?;
                at_most("base_char", m.base_char.len(), 4)?;
                hid("hid_hint", m.hid_hint)
            }
            Msg::KeyChar(m) => {
                at_most("text", m.text.len(), limits::KEY_TEXT_LEN)?;
                hid("hid_hint", m.hid_hint)
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
            Msg::StartVideo(StartVideo { wanted, .. })
            | Msg::SetVideoQuality(SetVideoQuality { wanted, .. }) => {
                quality_prefs(wanted.as_ref())
            }
            Msg::ConsentResult(_)
            | Msg::Ping(_)
            | Msg::Pong(_)
            | Msg::StopVideo(_)
            | Msg::SetPriority(_)
            | Msg::VideoStopped(_)
            | Msg::RequestKeyframe(_)
            | Msg::PictureLoss(_)
            | Msg::MouseMove(_)
            | Msg::MouseButton(_)
            | Msg::Scroll(_)
            | Msg::CursorShapeRef(_)
            | Msg::KeyboardMode(_)
            | Msg::ReleaseAll(_)
            | Msg::ClipboardRequest(_)
            | Msg::ClipboardError(_)
            | Msg::FsRootsRequest(_)
            | Msg::FileCancel(_)
            | Msg::FileError(_) => Ok(()),
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
        let teclado = |id: &str, tag: &str, extra: Vec<u32>| {
            Some(KeyboardInfo {
                os_layout_id: id.into(),
                layout_tag: tag.into(),
                extra_hid: extra,
            })
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
                keyboard: teclado("00020409", "us-intl", vec![]),
                password: "correta cavalo bateria".into(),
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
                keyboard: teclado("xkb:br(abnt2)", "br-abnt2", vec![0x87, 0x85]),
                password_enabled: true,
            }),
            Msg::KeyboardLayoutChanged(KeyboardLayoutChanged {
                info: teclado("00010416", "br-abnt2", vec![0x87, 0x85]),
            }),
            Msg::KeyboardMode(KeyboardMode {
                mode: KeyboardModeKind::Physical.into(),
            }),
            Msg::Key(Key {
                hid: 0x04,
                down: true,
                locks: Some(0b01),
            }),
            Msg::Key(Key {
                hid: 0xE1,
                down: false,
                locks: None,
            }),
            Msg::KeyShortcut(KeyShortcut {
                base_char: "ç".into(),
                hid_hint: 0x33,
                mods: 0b0010,
                down: true,
            }),
            Msg::KeyChar(KeyChar {
                text: "é".into(),
                hid_hint: 0x08,
            }),
            Msg::ReleaseAll(ReleaseAll {}),
            Msg::ConsentResult(ConsentResult {
                granted: true,
                permissions: Some(perms),
            }),
            Msg::Close(Close {
                reason: CloseReason::ByHost.into(),
                message: "tchau".into(),
            }),
            Msg::Close(Close {
                reason: CloseReason::ConsentTimeout.into(),
                message: String::new(),
            }),
            Msg::Close(Close {
                reason: CloseReason::NotAllowed.into(),
                message: String::new(),
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
                    codec: Codec::Hevc.into(),
                    bitrate_kbps: 0,
                }),
                priority: Priority::Focused.into(),
            }),
            Msg::SetVideoQuality(SetVideoQuality {
                display_id: 1,
                wanted: Some(QualityPrefs {
                    codec: Codec::PyroWave.into(),
                    bitrate_kbps: 20_000,
                    ..QualityPrefs::default()
                }),
            }),
            Msg::VideoStats(VideoStats {
                display_id: 1,
                encoder: "nvenc".into(),
                gpu: "NVIDIA GeForce RTX 3060 Laptop GPU".into(),
                fps_x100: 5994,
                bitrate_kbps: 7900,
                target_kbps: 8000,
                encode_us: 4300,
                skipped: 2,
                keyframes: 1,
                loss_ppm: 1500,
                width: 2560,
                height: 1600,
                est_kbps: 48_000,
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
            Msg::ClipboardOffer(ClipboardOffer {
                seq: 7,
                formats: vec![ClipFormat::Text.into(), ClipFormat::Files.into()],
                sizes: vec![12, 0],
                files: vec![
                    ClipFile {
                        name: "relatório.pdf".into(),
                        size: 1_048_576,
                        dir: false,
                    },
                    ClipFile {
                        name: "Fotos".into(),
                        size: 0,
                        dir: true,
                    },
                ],
            }),
            Msg::ClipboardRequest(ClipboardRequest {
                seq: 7,
                format: ClipFormat::Png.into(),
            }),
            Msg::ClipboardData(ClipboardData {
                seq: 7,
                format: ClipFormat::Text.into(),
                offset: 4,
                total: 12,
                data: b"mundo".to_vec(),
            }),
            Msg::ClipboardError(ClipboardError {
                seq: 6,
                format: ClipFormat::Html.into(),
                code: ClipErrorCode::Gone.into(),
            }),
            Msg::FsRootsRequest(FsRootsRequest {}),
            Msg::FsRoots(FsRoots {
                roots: vec![FsRoot {
                    id: 1,
                    name: "Documentos".into(),
                    access: FsAccess::ReadWrite.into(),
                }],
            }),
            Msg::FsList(FsList {
                req_id: 3,
                root: 1,
                path: "projetos/relatório".into(),
                start: 512,
            }),
            Msg::FsListing(FsListing {
                req_id: 3,
                entries: vec![
                    FsEntry {
                        name: "fotos".into(),
                        kind: FsEntryKind::Dir.into(),
                        size: 0,
                        mtime_ms: 1_760_000_000_000,
                    },
                    FsEntry {
                        name: "a.txt".into(),
                        kind: FsEntryKind::File.into(),
                        size: 12,
                        mtime_ms: 0,
                    },
                ],
                more: true,
                error: FileErrorCode::Unspecified.into(),
            }),
            Msg::FileOffer(FileOffer {
                xfer_id: 9,
                root: crate::files::INBOX_ROOT,
                rel_path: "pasta/vídeo.mkv".into(),
                size: 10 << 30,
                mtime_ms: 1_760_000_000_123,
            }),
            Msg::FileRequest(FileRequest {
                xfer_id: 10,
                root: 1,
                path: "a.txt".into(),
            }),
            Msg::FileAccept(FileAccept {
                xfer_id: 9,
                offset: 0,
                prefix_blake3: Vec::new(),
            }),
            Msg::FileAccept(FileAccept {
                xfer_id: 9,
                offset: 1 << 33,
                prefix_blake3: vec![7; 32],
            }),
            Msg::FileCancel(FileCancel { xfer_id: 9 }),
            Msg::FileDone(FileDone {
                xfer_id: 9,
                blake3: vec![0xab; 32],
            }),
            Msg::FileError(FileError {
                xfer_id: 9,
                code: FileErrorCode::ResumeMismatch.into(),
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
        // Campo 75 (faixa do clipboard, ainda sem mensagem), tipo length-delimited, vazio.
        let corpo = [(75 << 3 | 2) as u8 | 0x80, 75 >> 4, 0];
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
    fn limites_da_area_de_transferencia() {
        let pedaco = |offset: u64, n: usize, total: u64| {
            Msg::ClipboardData(ClipboardData {
                seq: 1,
                format: ClipFormat::Png.into(),
                offset,
                total,
                data: vec![0; n],
            })
        };
        let n = limits::CLIP_CHUNK;
        assert!(
            Control::from(pedaco(0, n, limits::CLIP_TOTAL))
                .validate()
                .is_ok()
        );
        assert_eq!(invalida(pedaco(0, n + 1, limits::CLIP_TOTAL)).field, "data");
        assert_eq!(
            invalida(pedaco(0, 1, limits::CLIP_TOTAL + 1)).field,
            "total"
        );
        // O pedaço passa do total anunciado.
        assert_eq!(invalida(pedaco(10, 5, 12)).field, "offset");
        let muitos = (0..=limits::CLIP_FILES)
            .map(|_| ClipFile {
                name: "a".into(),
                size: 1,
                dir: false,
            })
            .collect();
        let e = invalida(Msg::ClipboardOffer(ClipboardOffer {
            seq: 1,
            formats: vec![ClipFormat::Files.into()],
            sizes: vec![],
            files: muitos,
        }));
        assert_eq!(e.field, "files");
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
    fn taxa_fixa_tem_teto() {
        let prefs = |kbps| {
            Some(QualityPrefs {
                bitrate_kbps: kbps,
                ..QualityPrefs::default()
            })
        };
        let e = invalida(Msg::SetVideoQuality(SetVideoQuality {
            display_id: 0,
            wanted: prefs(limits::VIDEO_KBPS + 1),
        }));
        assert_eq!(e.field, "bitrate_kbps");
        let e = invalida(Msg::StartVideo(StartVideo {
            wanted: prefs(u32::MAX),
            ..StartVideo::default()
        }));
        assert_eq!(e.field, "bitrate_kbps");
        assert!(
            Control::from(Msg::SetVideoQuality(SetVideoQuality {
                display_id: 0,
                wanted: prefs(limits::VIDEO_KBPS),
            }))
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn limites_das_estatisticas_de_video() {
        let stats = |encoder: &str, gpu: &str| {
            Msg::VideoStats(VideoStats {
                encoder: encoder.into(),
                gpu: gpu.into(),
                ..Default::default()
            })
        };
        let gpu = "g".repeat(limits::SHORT_TEXT_LEN);
        assert!(Control::from(stats("vaapi", &gpu)).validate().is_ok());
        assert_eq!(invalida(stats(&"e".repeat(33), "")).field, "encoder");
        assert_eq!(invalida(stats("nvenc", &format!("{gpu}x"))).field, "gpu");
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
    fn limites_do_teclado() {
        let e = invalida(Msg::KeyChar(KeyChar {
            text: "a".repeat(limits::KEY_TEXT_LEN + 1),
            hid_hint: 0,
        }));
        assert_eq!(e.field, "text");
        let e = invalida(Msg::KeyShortcut(KeyShortcut {
            base_char: "ab".into(),
            hid_hint: 0x04,
            mods: 2,
            down: true,
        }));
        assert_eq!(e.field, "base_char");
        let e = invalida(Msg::Key(Key {
            hid: 0x1_0000,
            down: true,
            locks: None,
        }));
        assert_eq!(e.field, "hid");
        let e = invalida(Msg::KeyboardLayoutChanged(KeyboardLayoutChanged {
            info: Some(KeyboardInfo {
                os_layout_id: String::new(),
                layout_tag: String::new(),
                extra_hid: vec![0x87; limits::EXTRA_HID + 1],
            }),
        }));
        assert_eq!(e.field, "keyboard.extra_hid");
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
    fn senha_tem_limite_e_nao_aparece_no_debug() {
        let e = invalida(Msg::Hello(Hello {
            proto_version: PROTO_VERSION,
            password: "x".repeat(limits::PASSWORD_LEN + 1),
            ..Hello::default()
        }));
        assert_eq!(e.field, "password");
        let h = Hello {
            password: "segredo123".into(),
            ..Hello::default()
        };
        let d = format!("{h:?}");
        assert!(!d.contains("segredo123") && d.contains("***"), "{d}");
    }

    #[test]
    fn enum_desconhecido_vira_padrao() {
        let msg = SetPriority {
            display_id: 0,
            priority: 42,
        };
        assert_eq!(msg.priority(), Priority::Unspecified);
    }

    #[test]
    fn limites_de_arquivos() {
        let e = invalida(Msg::FileAccept(FileAccept {
            xfer_id: 1,
            offset: 5,
            prefix_blake3: Vec::new(),
        }));
        assert_eq!(e.field, "prefix_blake3");
        let e = invalida(Msg::FileAccept(FileAccept {
            xfer_id: 1,
            offset: 0,
            prefix_blake3: vec![0; 32],
        }));
        assert_eq!(e.field, "prefix_blake3");
        let e = invalida(Msg::FileDone(FileDone {
            xfer_id: 1,
            blake3: vec![0; 31],
        }));
        assert_eq!(e.field, "blake3");
        let e = invalida(Msg::FileRequest(FileRequest {
            xfer_id: 1,
            root: 1,
            path: "a".repeat(files::PATH_LEN + 1),
        }));
        assert_eq!(e.field, "path");
        let e = invalida(Msg::FsRoots(FsRoots {
            roots: vec![FsRoot {
                id: crate::files::INBOX_ROOT,
                name: String::new(),
                access: FsAccess::Read.into(),
            }],
        }));
        assert_eq!(e.field, "root.id");
    }

    #[test]
    fn pagina_cheia_de_listagem_cabe_num_frame() {
        let entries = (0..files::LISTING_ENTRIES)
            .map(|_| FsEntry {
                name: "n".repeat(files::LISTING_BYTES / files::LISTING_ENTRIES),
                kind: FsEntryKind::File.into(),
                size: u64::MAX,
                mtime_ms: i64::MIN,
            })
            .collect();
        let msg = Control::from(Msg::FsListing(FsListing {
            req_id: u32::MAX,
            entries,
            more: true,
            error: 0,
        }));
        assert!(
            msg.encoded_len() <= limits::FRAME_LEN,
            "{}",
            msg.encoded_len()
        );
    }
}
