//! Escolha do codec e do backend de vídeo: hardware sempre que possível, com probe e fallback.
//!
//! Codec: o controlador diz o que decodifica no `Hello` ([`codecs_para_decodificar`]: H.264
//! sempre, HEVC e AV1 só com decoder de hardware) e o host fica com o melhor que ele também
//! codifica em hardware, na ordem AV1, HEVC, H.264 ([`candidatos_encoder`]). `TAILDESK_CODEC`
//! (ex.: `h264`, `hevc,h264`) muda essa ordem.
//!
//! Encoder: NVENC primeiro (num i7-12700H com Iris Xe e RTX 3060, na bateria, 1080p sai em 7 ms
//! no NVENC e em 22 ms no VA-API da Iris Xe), depois o VA-API da GPU ligada às telas, as outras
//! GPUs e o openh264 por último. Decoder: ver [`opcoes_decoder`].
//!
//! `TAILDESK_ENCODER` e `TAILDESK_DECODER` (ex.: `vaapi`, `nvenc,openh264`) restringem e
//! reordenam as listas pelo nome do backend, para diagnóstico.

use crate::{
    CodecVideo, ConfigEncoder, DecoderOpenH264, EncoderOpenH264, ErroCodec, VideoDecoder,
    VideoEncoder,
};

/// Onde um backend roda.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Onde {
    Cpu,
    #[cfg(any(target_os = "linux", windows))]
    Nvidia(crate::nvidia::cuda::CuDevice),
    #[cfg(target_os = "linux")]
    Va(std::path::PathBuf),
    /// Media Foundation (o decoder do sistema, na GPU quando há D3D11).
    #[cfg(windows)]
    Mf,
    /// PyroWave em compute Vulkan.
    #[cfg(any(target_os = "linux", windows))]
    PyroWave,
}

/// Um backend que dá para abrir nesta máquina, para um codec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opcao {
    /// `openh264`, `nvenc`, `vaapi`, `nvdec`, `mf`.
    pub backend: &'static str,
    /// Nome da GPU (`None` = CPU).
    pub gpu: Option<String>,
    pub codec: CodecVideo,
    onde: Onde,
}

impl Opcao {
    pub fn hardware(&self) -> bool {
        self.onde != Onde::Cpu
    }

    /// Abre o encoder desta opção (com o quadro de teste dos backends de hardware).
    pub fn abrir_encoder(&self, config: ConfigEncoder) -> Result<Box<dyn VideoEncoder>, ErroCodec> {
        match &self.onde {
            Onde::Cpu => Ok(Box::new(EncoderOpenH264::novo(config)?)),
            #[cfg(any(target_os = "linux", windows))]
            Onde::Nvidia(dev) => Ok(Box::new(crate::nvidia::nvenc::EncoderNvenc::novo(
                *dev,
                self.gpu.clone().unwrap_or_default(),
                self.codec,
                config,
            )?)),
            #[cfg(target_os = "linux")]
            Onde::Va(no) => Ok(Box::new(crate::linux::vaapi::EncoderVaapi::novo(
                no,
                self.gpu.clone().unwrap_or_default(),
                config,
            )?)),
            #[cfg(windows)]
            Onde::Mf => Err(ErroCodec::Backend("mf: encoder indisponível".into())),
            #[cfg(any(target_os = "linux", windows))]
            Onde::PyroWave => Ok(Box::new(crate::pyrowave::EncoderPyroWave::novo(config)?)),
        }
    }

    /// Abre o decoder desta opção.
    pub fn abrir_decoder(&self) -> Result<Box<dyn VideoDecoder>, ErroCodec> {
        match &self.onde {
            Onde::Cpu => Ok(Box::new(DecoderOpenH264::novo()?)),
            #[cfg(any(target_os = "linux", windows))]
            Onde::Nvidia(dev) => Ok(Box::new(crate::nvidia::nvdec::DecoderNvdec::novo(
                *dev,
                self.gpu.clone().unwrap_or_default(),
                self.codec,
            )?)),
            #[cfg(target_os = "linux")]
            Onde::Va(_) => Err(ErroCodec::Backend("vaapi: decoder indisponível".into())),
            #[cfg(windows)]
            Onde::Mf => Ok(Box::new(crate::windows::mf::DecoderMf::novo(self.codec)?)),
            #[cfg(any(target_os = "linux", windows))]
            Onde::PyroWave => Ok(Box::new(crate::pyrowave::DecoderPyroWave::novo()?)),
        }
    }
}

#[cfg(any(target_os = "linux", windows))]
fn pyrowave() -> Opcao {
    Opcao {
        backend: "pyrowave",
        gpu: Some("Vulkan".into()),
        codec: CodecVideo::PyroWave,
        onde: Onde::PyroWave,
    }
}

fn openh264() -> Opcao {
    Opcao {
        backend: "openh264",
        gpu: None,
        codec: CodecVideo::H264,
        onde: Onde::Cpu,
    }
}

/// Nome do codec em `TAILDESK_CODEC` e nos logs.
pub fn nome_codec(c: CodecVideo) -> &'static str {
    match c {
        CodecVideo::H264 => "h264",
        CodecVideo::Hevc => "hevc",
        CodecVideo::Av1 => "av1",
        CodecVideo::PyroWave => "pyrowave",
    }
}

/// Lista de nomes de uma variável de ambiente (`a,b,c`); vazia se não existir.
fn nomes(variavel: &str) -> Vec<String> {
    std::env::var(variavel)
        .unwrap_or_default()
        .split(',')
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Aplica `TAILDESK_ENCODER`/`TAILDESK_DECODER`: só os backends citados, na ordem citada.
fn filtrar(mut opcoes: Vec<Opcao>, variavel: &str) -> Vec<Opcao> {
    let nomes = nomes(variavel);
    if nomes.is_empty() {
        return opcoes;
    }
    opcoes.retain(|o| nomes.iter().any(|n| n == o.backend));
    opcoes.sort_by_key(|o| nomes.iter().position(|n| n == o.backend));
    opcoes
}

/// Encoders desta máquina para `codec`, em ordem de preferência (ver o topo do módulo).
pub fn opcoes_encoder(codec: CodecVideo) -> Vec<Opcao> {
    let mut opcoes: Vec<Opcao> = encoders_de_hardware()
        .into_iter()
        .filter(|o| o.codec == codec)
        .collect();
    #[cfg(any(target_os = "linux", windows))]
    if codec == CodecVideo::PyroWave && crate::pyrowave::disponivel() {
        opcoes.push(pyrowave());
    }
    if codec == CodecVideo::H264 {
        opcoes.push(openh264());
    }
    filtrar(opcoes, "TAILDESK_ENCODER")
}

/// Encoders para os codecs que o controlador decodifica (`aceitos`; vazio = só H.264), o
/// melhor codec primeiro. HEVC e AV1 só entram com encoder de hardware. O PyroWave nunca entra
/// sozinho (pede centenas de Mbit/s): só se for pedido em `TAILDESK_CODEC` ou pela escolha
/// manual do controlador, que usa [`opcoes_encoder`] direto.
pub fn candidatos_encoder(aceitos: &[CodecVideo]) -> Vec<Opcao> {
    let mut ordem = vec![CodecVideo::Av1, CodecVideo::Hevc, CodecVideo::H264];
    let forcados = nomes("TAILDESK_CODEC");
    if !forcados.is_empty() {
        ordem.push(CodecVideo::PyroWave);
        ordem.retain(|c| forcados.iter().any(|n| n == nome_codec(*c)));
        ordem.sort_by_key(|c| forcados.iter().position(|n| n == nome_codec(*c)));
    }
    ordem
        .into_iter()
        .filter(|c| *c == CodecVideo::H264 && aceitos.is_empty() || aceitos.contains(c))
        .flat_map(opcoes_encoder)
        .collect()
}

#[cfg(not(any(target_os = "linux", windows)))]
fn encoders_de_hardware() -> Vec<Opcao> {
    Vec::new()
}

/// NVENC de uma GPU CUDA: uma opção por codec que ela codifica.
#[cfg(any(target_os = "linux", windows))]
fn nvenc_de(dev: crate::nvidia::cuda::CuDevice, nome: &str) -> Vec<Opcao> {
    crate::nvidia::nvenc::codecs(dev)
        .into_iter()
        .map(|codec| Opcao {
            backend: "nvenc",
            gpu: Some(nome.to_owned()),
            codec,
            onde: Onde::Nvidia(dev),
        })
        .collect()
}

/// Windows: NVENC em cada GPU NVIDIA. Intel e AMD (Media Foundation) ainda não.
#[cfg(windows)]
fn encoders_de_hardware() -> Vec<Opcao> {
    crate::nvidia::nvenc::gpus()
        .iter()
        .flat_map(|(dev, nome, _)| nvenc_de(*dev, nome))
        .collect()
}

#[cfg(target_os = "linux")]
fn encoders_de_hardware() -> Vec<Opcao> {
    use crate::linux::{gpu, vaapi};
    use crate::nvidia::nvenc;
    let mut opcoes = Vec::new();
    let nvidia = nvenc::gpus();
    for g in gpu::gpus() {
        if g.fabricante == gpu::NVIDIA {
            // O VA-API da NVIDIA (nvidia-vaapi-driver) não codifica: só o NVENC.
            if let Some((dev, nome, _)) = nvidia.iter().find(|(_, _, pci)| *pci == g.pci) {
                opcoes.extend(nvenc_de(*dev, nome));
            }
        } else if vaapi::codifica_h264(&g.no).is_some() {
            opcoes.push(Opcao {
                backend: "vaapi",
                gpu: Some(g.nome.clone()),
                codec: CodecVideo::H264,
                onde: Onde::Va(g.no.clone()),
            });
        }
    }
    // NVENC de uma GPU sem nó de render visível (container, driver sem DRM).
    for (dev, nome, _) in &nvidia {
        if !opcoes.iter().any(|o| o.onde == Onde::Nvidia(*dev)) {
            opcoes.extend(nvenc_de(*dev, nome));
        }
    }
    // NVENC antes do VA-API (ver o topo do módulo); `sort_by_key` é estável.
    opcoes.sort_by_key(|o| o.backend != "nvenc");
    opcoes
}

/// Decoders desta máquina para `codec`, em ordem de preferência.
///
/// No Linux o openh264 vem antes do NVDEC para H.264: com SIMD ele decodifica 1920x1200 em
/// ~1,5 ms, e o NVDEC leva 2,4 ms ou mais porque o quadro volta da GPU para a CPU antes de ir
/// para o render (medido com o `medir_encode` num i7-12700H com RTX 3060). No Windows o Media
/// Foundation com D3D11 vem primeiro.
pub fn opcoes_decoder(codec: CodecVideo) -> Vec<Opcao> {
    let hardware = decoders_de_hardware()
        .into_iter()
        .filter(|o| o.codec == codec);
    let mut opcoes = Vec::new();
    if codec == CodecVideo::H264 && !cfg!(windows) {
        opcoes.push(openh264());
    }
    opcoes.extend(hardware);
    if codec == CodecVideo::H264 && cfg!(windows) {
        opcoes.push(openh264());
    }
    #[cfg(any(target_os = "linux", windows))]
    if codec == CodecVideo::PyroWave && crate::pyrowave::disponivel() {
        opcoes.push(pyrowave());
    }
    filtrar(opcoes, "TAILDESK_DECODER")
}

/// Codecs que este controlador anuncia no `Hello`: H.264 sempre (openh264), HEVC e AV1 quando
/// há decoder de hardware para eles.
pub fn codecs_para_decodificar() -> Vec<CodecVideo> {
    let mut codecs = vec![CodecVideo::H264];
    for c in [CodecVideo::Hevc, CodecVideo::Av1, CodecVideo::PyroWave] {
        if opcoes_decoder(c).iter().any(Opcao::hardware) {
            codecs.push(c);
        }
    }
    codecs
}

#[cfg(not(any(target_os = "linux", windows)))]
fn decoders_de_hardware() -> Vec<Opcao> {
    Vec::new()
}

/// Windows: Media Foundation com D3D11 primeiro, NVDEC depois.
#[cfg(windows)]
fn decoders_de_hardware() -> Vec<Opcao> {
    use crate::windows::mf;
    let mut opcoes: Vec<Opcao> = [CodecVideo::H264, CodecVideo::Hevc, CodecVideo::Av1]
        .into_iter()
        .filter_map(|codec| {
            mf::gpu_do_decoder(codec).map(|gpu| Opcao {
                backend: "mf",
                gpu: Some(gpu),
                codec,
                onde: Onde::Mf,
            })
        })
        .collect();
    for (dev, nome, _) in crate::nvidia::nvdec::gpus() {
        opcoes.extend(
            crate::nvidia::nvdec::codecs(dev)
                .into_iter()
                .map(|codec| Opcao {
                    backend: "nvdec",
                    gpu: Some(nome.clone()),
                    codec,
                    onde: Onde::Nvidia(dev),
                }),
        );
    }
    opcoes
}

#[cfg(target_os = "linux")]
fn decoders_de_hardware() -> Vec<Opcao> {
    use crate::linux::gpu;
    use crate::nvidia::nvdec;
    let mut nvidia = nvdec::gpus();
    let mut ordem = Vec::new();
    // GPUs na ordem de preferência (a ligada às telas primeiro), depois as sem nó de render.
    for g in gpu::gpus().iter().filter(|g| g.fabricante == gpu::NVIDIA) {
        if let Some(i) = nvidia.iter().position(|(_, _, pci)| *pci == g.pci) {
            ordem.push(nvidia.remove(i));
        }
    }
    ordem.extend(nvidia);
    ordem
        .into_iter()
        .flat_map(|(dev, nome, _)| {
            nvdec::codecs(dev).into_iter().map(move |codec| Opcao {
                backend: "nvdec",
                gpu: Some(nome.clone()),
                codec,
                onde: Onde::Nvidia(dev),
            })
        })
        .collect()
}

/// Abre o primeiro decoder de `codec` que funcionar. Os que falharam vão para `falhas`
/// (backend e erro), para o log.
pub fn abrir_decoder(
    codec: CodecVideo,
    falhas: &mut Vec<(Opcao, ErroCodec)>,
) -> Result<Box<dyn VideoDecoder>, ErroCodec> {
    let mut ultimo = ErroCodec::Backend(format!("sem decoder de {}", nome_codec(codec)));
    for o in opcoes_decoder(codec) {
        match o.abrir_decoder() {
            Ok(c) => return Ok(c),
            Err(e) => {
                ultimo = ErroCodec::Backend(e.to_string());
                falhas.push((o, e));
            }
        }
    }
    Err(ultimo)
}

/// Relatório do que esta máquina codifica e decodifica, com o erro de cada backend que não abriu
/// (`taildesk-cli codecs`). Abre cada encoder e decoder de verdade.
pub fn relatorio() -> String {
    use std::fmt::Write;
    let gpu = |o: &Opcao| o.gpu.clone().unwrap_or_else(|| "CPU".into());
    let mut r = String::new();
    let codecs = [
        CodecVideo::H264,
        CodecVideo::Hevc,
        CodecVideo::Av1,
        CodecVideo::PyroWave,
    ];
    let _ = writeln!(r, "Encoders:");
    for c in codecs {
        let opcoes = opcoes_encoder(c);
        if opcoes.is_empty() {
            let _ = writeln!(r, "  {}: nenhum", nome_codec(c));
        }
        for o in opcoes {
            let config = ConfigEncoder {
                bitrate_bps: 4_000_000,
                fps_max: 60.0,
            };
            let estado = match o.abrir_encoder(config) {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("falhou: {e}"),
            };
            let _ = writeln!(
                r,
                "  {} {} ({}): {estado}",
                nome_codec(c),
                o.backend,
                gpu(&o)
            );
        }
    }
    let _ = writeln!(r, "Decoders:");
    for c in codecs {
        let opcoes = opcoes_decoder(c);
        if opcoes.is_empty() {
            let _ = writeln!(r, "  {}: nenhum de hardware", nome_codec(c));
        }
        for o in opcoes {
            let estado = match o.abrir_decoder() {
                Ok(d) => format!("ok ({} {})", d.nome(), d.gpu().unwrap_or("CPU")),
                Err(e) => format!("falhou: {e}"),
            };
            let _ = writeln!(
                r,
                "  {} {} ({}): {estado}",
                nome_codec(c),
                o.backend,
                gpu(&o)
            );
        }
        #[cfg(windows)]
        if let Err(e) = crate::windows::mf::sondar(c) {
            let _ = writeln!(r, "  {} mf: {e}", nome_codec(c));
        }
    }
    let anunciados: Vec<&str> = codecs_para_decodificar()
        .into_iter()
        .map(nome_codec)
        .collect();
    let _ = writeln!(r, "Anunciados no Hello: {}", anunciados.join(", "));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openh264_por_ultimo_no_h264() {
        let opcoes = opcoes_encoder(CodecVideo::H264);
        assert_eq!(opcoes.last(), Some(&openh264()));
        assert!(opcoes.iter().rev().skip(1).all(Opcao::hardware));
        assert!(opcoes_encoder(CodecVideo::Hevc).iter().all(Opcao::hardware));
    }

    #[test]
    fn controlador_antigo_recebe_h264() {
        assert!(
            candidatos_encoder(&[])
                .iter()
                .all(|o| o.codec == CodecVideo::H264)
        );
        assert!(codecs_para_decodificar().contains(&CodecVideo::H264));
    }

    #[test]
    fn abre_encoder_e_decoder_de_cada_codec() {
        let aceitos = codecs_para_decodificar();
        eprintln!("decodifica: {aceitos:?}");
        for o in candidatos_encoder(&aceitos) {
            let config = ConfigEncoder {
                bitrate_bps: 2_000_000,
                fps_max: 30.0,
            };
            match o.abrir_encoder(config) {
                Ok(e) => eprintln!("encoder {:?}: {} {:?}", e.codec(), e.nome(), e.gpu()),
                Err(e) => eprintln!("encoder {} {:?} falhou: {e}", o.backend, o.codec),
            }
        }
        for c in aceitos {
            let mut falhas = Vec::new();
            let d = abrir_decoder(c, &mut falhas).unwrap();
            eprintln!("decoder {c:?}: {} {:?}", d.nome(), d.gpu());
        }
    }
}
