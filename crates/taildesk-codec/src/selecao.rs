//! Escolha do backend de vídeo: hardware sempre que possível, com probe e fallback.
//!
//! [`opcoes_encoder`] lista o que dá para usar nesta máquina, em ordem de preferência: NVENC
//! primeiro (num i7-12700H com Iris Xe e RTX 3060, na bateria, 1080p sai em 7 ms no NVENC e em
//! 22 ms no VA-API da Iris Xe), depois o VA-API da GPU ligada às telas, as outras GPUs e o
//! openh264 por último. [`abrir_encoder`] tenta cada opção até uma abrir e
//! codificar o quadro de teste. Os decoders seguem outra ordem (ver [`opcoes_decoder`]).
//!
//! `TAILDESK_ENCODER` e `TAILDESK_DECODER` (ex.: `vaapi`, `nvenc,openh264`) restringem e
//! reordenam a lista pelo nome do backend, para diagnóstico.

use crate::{
    ConfigEncoder, DecoderOpenH264, EncoderOpenH264, ErroCodec, VideoDecoder, VideoEncoder,
};

/// Onde um backend roda.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Onde {
    Cpu,
    #[cfg(target_os = "linux")]
    Nvidia(crate::linux::cuda::CuDevice),
    #[cfg(target_os = "linux")]
    Va(std::path::PathBuf),
}

/// Um backend que dá para abrir nesta máquina.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opcao {
    /// `openh264`, `nvenc`, `vaapi`, `nvdec`.
    pub backend: &'static str,
    /// Nome da GPU (`None` = CPU).
    pub gpu: Option<String>,
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
            #[cfg(target_os = "linux")]
            Onde::Nvidia(dev) => Ok(Box::new(crate::linux::nvenc::EncoderNvenc::novo(
                *dev,
                self.gpu.clone().unwrap_or_default(),
                config,
            )?)),
            #[cfg(target_os = "linux")]
            Onde::Va(no) => Ok(Box::new(crate::linux::vaapi::EncoderVaapi::novo(
                no,
                self.gpu.clone().unwrap_or_default(),
                config,
            )?)),
        }
    }

    /// Abre o decoder desta opção.
    pub fn abrir_decoder(&self) -> Result<Box<dyn VideoDecoder>, ErroCodec> {
        match &self.onde {
            Onde::Cpu => Ok(Box::new(DecoderOpenH264::novo()?)),
            #[cfg(target_os = "linux")]
            Onde::Nvidia(dev) => Ok(Box::new(crate::linux::nvdec::DecoderNvdec::novo(
                *dev,
                self.gpu.clone().unwrap_or_default(),
            )?)),
            #[cfg(target_os = "linux")]
            Onde::Va(_) => Err(ErroCodec::Backend(format!(
                "{}: decoder indisponível",
                self.backend
            ))),
        }
    }
}

fn openh264() -> Opcao {
    Opcao {
        backend: "openh264",
        gpu: None,
        onde: Onde::Cpu,
    }
}

/// Aplica `TAILDESK_ENCODER`/`TAILDESK_DECODER`: só os backends citados, na ordem citada.
fn filtrar(mut opcoes: Vec<Opcao>, variavel: &str) -> Vec<Opcao> {
    let Ok(lista) = std::env::var(variavel) else {
        return opcoes;
    };
    let nomes: Vec<&str> = lista
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    if nomes.is_empty() {
        return opcoes;
    }
    opcoes.retain(|o| nomes.contains(&o.backend));
    opcoes.sort_by_key(|o| nomes.iter().position(|n| *n == o.backend));
    opcoes
}

/// Encoders desta máquina, em ordem de preferência (ver o topo do módulo).
pub fn opcoes_encoder() -> Vec<Opcao> {
    let mut opcoes = encoders_de_hardware();
    opcoes.push(openh264());
    filtrar(opcoes, "TAILDESK_ENCODER")
}

#[cfg(not(target_os = "linux"))]
fn encoders_de_hardware() -> Vec<Opcao> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn encoders_de_hardware() -> Vec<Opcao> {
    use crate::linux::{gpu, nvenc, vaapi};
    let mut opcoes = Vec::new();
    let nvidia = nvenc::gpus();
    for g in gpu::gpus() {
        if g.fabricante == gpu::NVIDIA {
            // O VA-API da NVIDIA (nvidia-vaapi-driver) não codifica: só o NVENC.
            if let Some((dev, nome, _)) = nvidia.iter().find(|(_, _, pci)| *pci == g.pci) {
                opcoes.push(Opcao {
                    backend: "nvenc",
                    gpu: Some(nome.clone()),
                    onde: Onde::Nvidia(*dev),
                });
            }
        } else if vaapi::codifica_h264(&g.no).is_some() {
            opcoes.push(Opcao {
                backend: "vaapi",
                gpu: Some(g.nome.clone()),
                onde: Onde::Va(g.no.clone()),
            });
        }
    }
    // NVENC de uma GPU sem nó de render visível (container, driver sem DRM).
    for (dev, nome, _) in nvidia {
        if !opcoes.iter().any(|o| o.onde == Onde::Nvidia(dev)) {
            opcoes.push(Opcao {
                backend: "nvenc",
                gpu: Some(nome),
                onde: Onde::Nvidia(dev),
            });
        }
    }
    // NVENC antes do VA-API (ver o topo do módulo); `sort_by_key` é estável.
    opcoes.sort_by_key(|o| o.backend != "nvenc");
    opcoes
}

/// Decoders desta máquina, em ordem de preferência.
///
/// O openh264 vem primeiro: com SIMD ele decodifica 1920x1200 em ~1,5 ms, e o NVDEC leva 2,4 ms
/// ou mais porque o quadro volta da GPU para a CPU antes de ir para o render (medido com o
/// `medir_encode` num i7-12700H com RTX 3060). O hardware passa na frente quando o quadro for
/// direto do decoder para a textura do wgpu.
pub fn opcoes_decoder() -> Vec<Opcao> {
    let mut opcoes = vec![openh264()];
    opcoes.extend(decoders_de_hardware());
    filtrar(opcoes, "TAILDESK_DECODER")
}

#[cfg(not(target_os = "linux"))]
fn decoders_de_hardware() -> Vec<Opcao> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn decoders_de_hardware() -> Vec<Opcao> {
    use crate::linux::{gpu, nvdec};
    let mut nvidia = nvdec::gpus();
    let mut opcoes = Vec::new();
    // GPUs na ordem de preferência (a ligada às telas primeiro), depois as sem nó de render.
    for g in gpu::gpus().iter().filter(|g| g.fabricante == gpu::NVIDIA) {
        if let Some(i) = nvidia.iter().position(|(_, _, pci)| *pci == g.pci) {
            let (dev, nome, _) = nvidia.remove(i);
            opcoes.push(Opcao {
                backend: "nvdec",
                gpu: Some(nome),
                onde: Onde::Nvidia(dev),
            });
        }
    }
    opcoes.extend(nvidia.into_iter().map(|(dev, nome, _)| Opcao {
        backend: "nvdec",
        gpu: Some(nome),
        onde: Onde::Nvidia(dev),
    }));
    opcoes
}

/// Abre o primeiro encoder que funcionar. Os que falharam vão para `falhas` (backend e erro),
/// para o log e para o painel de diagnóstico.
pub fn abrir_encoder(
    config: ConfigEncoder,
    falhas: &mut Vec<(Opcao, ErroCodec)>,
) -> Result<Box<dyn VideoEncoder>, ErroCodec> {
    abrir(opcoes_encoder(), falhas, |o| o.abrir_encoder(config))
}

/// Abre o primeiro decoder que funcionar (ver [`abrir_encoder`]).
pub fn abrir_decoder(
    falhas: &mut Vec<(Opcao, ErroCodec)>,
) -> Result<Box<dyn VideoDecoder>, ErroCodec> {
    abrir(opcoes_decoder(), falhas, Opcao::abrir_decoder)
}

fn abrir<T>(
    opcoes: Vec<Opcao>,
    falhas: &mut Vec<(Opcao, ErroCodec)>,
    abrir: impl Fn(&Opcao) -> Result<T, ErroCodec>,
) -> Result<T, ErroCodec> {
    let mut ultimo = ErroCodec::Backend("nenhum backend de vídeo disponível".into());
    for o in opcoes {
        match abrir(&o) {
            Ok(c) => return Ok(c),
            Err(e) => {
                ultimo = ErroCodec::Backend(e.to_string());
                falhas.push((o, e));
            }
        }
    }
    Err(ultimo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openh264_sempre_por_ultimo() {
        let opcoes = opcoes_encoder();
        assert_eq!(opcoes.last(), Some(&openh264()));
        assert!(opcoes.iter().rev().skip(1).all(Opcao::hardware));
    }

    #[test]
    fn abre_algum_encoder_e_decoder() {
        let mut falhas = Vec::new();
        let enc = abrir_encoder(
            ConfigEncoder {
                bitrate_bps: 2_000_000,
                fps_max: 30.0,
            },
            &mut falhas,
        )
        .unwrap();
        for (o, e) in &falhas {
            eprintln!("{} ({:?}) falhou: {e}", o.backend, o.gpu);
        }
        eprintln!("encoder: {} {:?}", enc.nome(), enc.gpu());
        let dec = abrir_decoder(&mut falhas).unwrap();
        eprintln!("decoder: {} {:?}", dec.nome(), dec.gpu());
    }
}
