//! Escolha do backend de vídeo: hardware sempre que possível, com probe e fallback.
//!
//! [`opcoes_encoder`] lista o que dá para usar nesta máquina, em ordem de preferência: primeiro a
//! GPU ligada às telas (onde a imagem já está e que costuma ser a integrada, que gasta menos),
//! depois as outras, e o openh264 por último. Numa mesma GPU NVIDIA o NVENC vem antes do
//! VA-API. [`abrir_encoder`] tenta cada opção até uma abrir e codificar o quadro de teste.
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
            _ => Err(ErroCodec::Backend(format!(
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
    #[allow(unused_mut)]
    let mut opcoes = Vec::new();
    #[cfg(target_os = "linux")]
    {
        use crate::linux::{gpu, nvenc, vaapi};
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
    }
    opcoes.push(openh264());
    filtrar(opcoes, "TAILDESK_ENCODER")
}

/// Decoders desta máquina, em ordem de preferência.
pub fn opcoes_decoder() -> Vec<Opcao> {
    filtrar(vec![openh264()], "TAILDESK_DECODER")
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
        assert!(abrir_decoder(&mut falhas).is_ok());
    }
}
