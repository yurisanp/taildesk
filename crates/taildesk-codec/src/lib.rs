//! Codecs de vídeo: traits [`VideoEncoder`]/[`VideoDecoder`], conversão de cor e os backends:
//! NVENC e VA-API no Linux, por hardware, e o openh264 em software nos dois sistemas. A escolha
//! ([`abrir_encoder`]) prefere hardware, com probe e fallback até o openh264.
//!
//! O encoder recebe o quadro da captura (BGRx/RGBx com stride) e devolve um access unit em
//! Annex-B num buffer do chamador; o decoder recebe o access unit e empresta os planos I420 a um
//! callback. Nada aloca por quadro depois do primeiro.

use std::fmt;

pub mod cor;
#[cfg(target_os = "linux")]
mod h264;
#[cfg(target_os = "linux")]
pub mod linux;
mod openh264;
mod selecao;

pub use crate::openh264::{DecoderOpenH264, EncoderOpenH264};
pub use crate::selecao::{Opcao, abrir_decoder, abrir_encoder, opcoes_decoder, opcoes_encoder};

/// Ordem dos bytes de cada pixel da imagem capturada (4 bytes por pixel, o 4º ignorado).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrdemRgb {
    Bgrx,
    Rgbx,
}

/// Imagem a codificar, emprestada da captura.
#[derive(Clone, Copy, Debug)]
pub struct QuadroRgb<'a> {
    pub largura: u32,
    pub altura: u32,
    /// Bytes por linha (≥ `largura * 4`).
    pub stride: usize,
    pub ordem: OrdemRgb,
    pub dados: &'a [u8],
}

impl QuadroRgb<'_> {
    /// `dados` cobre `altura` linhas de `stride` bytes (a última pode ter só `largura * 4`).
    pub fn valido(&self) -> bool {
        let linha = self.largura as usize * 4;
        self.largura >= 2
            && self.altura >= 2
            && self.stride >= linha
            && (self.altura as usize - 1)
                .checked_mul(self.stride)
                .and_then(|n| n.checked_add(linha))
                .is_some_and(|n| n <= self.dados.len())
    }
}

/// Parâmetros do encoder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfigEncoder {
    pub bitrate_bps: u32,
    pub fps_max: f32,
}

/// Tipo do quadro codificado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipoQuadro {
    /// IDR: o decoder começa daqui.
    Chave,
    Delta,
}

/// Formato dos planos decodificados.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatoYuv {
    /// Y + U + V (3 planos, croma com metade da largura e da altura).
    I420,
    /// Y + UV intercalados (2 planos).
    Nv12,
}

/// Matriz de cor do vídeo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matriz {
    Bt601,
    Bt709,
}

/// Faixa dos valores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Faixa {
    Limitada,
    Cheia,
}

/// Um plano decodificado.
#[derive(Clone, Copy, Debug)]
pub struct PlanoYuv<'a> {
    pub dados: &'a [u8],
    pub passo: u32,
}

/// Quadro decodificado, emprestado do decoder durante o callback.
#[derive(Clone, Copy, Debug)]
pub struct QuadroYuv<'a> {
    pub largura: u32,
    pub altura: u32,
    pub formato: FormatoYuv,
    /// 3 planos em I420; em NV12 o terceiro vem vazio.
    pub planos: [PlanoYuv<'a>; 3],
    pub matriz: Matriz,
    pub faixa: Faixa,
}

/// Erro de um codec.
#[derive(Debug)]
pub enum ErroCodec {
    /// Imagem com tamanho, stride ou buffer inconsistentes.
    QuadroInvalido,
    /// Erro do backend (openh264, MF, VA-API).
    Backend(String),
}

impl fmt::Display for ErroCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErroCodec::QuadroInvalido => f.write_str("quadro de vídeo inválido"),
            ErroCodec::Backend(e) => write!(f, "codec: {e}"),
        }
    }
}

impl std::error::Error for ErroCodec {}

/// Encoder de vídeo de um monitor.
pub trait VideoEncoder: Send {
    /// Nome para logs e para o teste de conexão (`openh264`, `mf`, `vaapi`, `nvenc`).
    fn nome(&self) -> &'static str;

    /// GPU que codifica (`None` = CPU).
    fn gpu(&self) -> Option<&str> {
        None
    }

    /// Codifica `quadro` e escreve o access unit (Annex-B) em `saida`, que é limpo antes.
    /// `forcar_chave` pede um IDR (início, perda de pacotes). `Ok(None)` = o controle de taxa
    /// pulou o quadro e `saida` fica vazio. Mudança de tamanho reconfigura o encoder.
    fn codificar(
        &mut self,
        quadro: &QuadroRgb<'_>,
        forcar_chave: bool,
        saida: &mut Vec<u8>,
    ) -> Result<Option<TipoQuadro>, ErroCodec>;

    /// Novo alvo de taxa; vale a partir do próximo quadro.
    fn ajustar_bitrate(&mut self, bps: u32);
}

/// Decoder de vídeo de um monitor.
pub trait VideoDecoder: Send {
    fn nome(&self) -> &'static str;

    /// GPU que decodifica (`None` = CPU).
    fn gpu(&self) -> Option<&str> {
        None
    }

    /// Decodifica um access unit e chama `ao_quadro` se saiu imagem. `Ok(false)` = ainda sem
    /// imagem (o decoder espera um IDR).
    fn decodificar(
        &mut self,
        access_unit: &[u8],
        ao_quadro: &mut dyn FnMut(&QuadroYuv<'_>),
    ) -> Result<bool, ErroCodec>;
}
