//! Superfície de vídeo em `wgpu` (`10-plano-windows-linux.md` §1.4): o quadro decodificado chega
//! em YUV 4:2:0 (NV12 dos decoders de hardware, I420 do openh264), sobe para a GPU por plano e
//! vira uma textura RGBA que o Slint mostra direto (`slint::Image::try_from(wgpu::Texture)`).
//!
//! Um [`Conversor`] por `wgpu::Device` (o mesmo device em todas as janelas, como o Slint usa com
//! `WGPUConfiguration::Manual`) e uma [`Superficie`] por monitor remoto. O cursor do host vem à
//! parte (`taildesk_session::cursor`) e é desenhado pela UI por cima do vídeo.

use wgpu::util::DeviceExt;

pub use wgpu;

/// Formato da textura de saída, o que o Slint aceita.
pub const FORMATO_SAIDA: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Como os planos chegam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Formato {
    /// Y + CbCr intercalados (2 planos).
    Nv12,
    /// Y + Cb + Cr (3 planos).
    I420,
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
    /// Y 16..=235, C 16..=240 (o padrão do H.264).
    Limitada,
    Cheia,
}

/// Linhas da matriz YCbCr -> RGB e o deslocamento, no formato do uniform do shader.
fn coeficientes(matriz: Matriz, faixa: Faixa) -> [[f32; 4]; 4] {
    let (kr_cr, kg_cb, kg_cr, kb_cb) = match matriz {
        Matriz::Bt601 => (1.402, -0.344_136, -0.714_136, 1.772),
        Matriz::Bt709 => (1.5748, -0.187_324, -0.468_124, 1.8556),
    };
    let (sy, sc, oy) = match faixa {
        Faixa::Limitada => (255.0 / 219.0, 255.0 / 224.0, 16.0 / 255.0),
        Faixa::Cheia => (1.0, 1.0, 0.0),
    };
    [
        [sy, 0.0, kr_cr * sc, 0.0],
        [sy, kg_cb * sc, kg_cr * sc, 0.0],
        [sy, kb_cb * sc, 0.0, 0.0],
        [oy, 128.0 / 255.0, 128.0 / 255.0, 1.0],
    ]
}

/// Converte um pixel na CPU, igual ao shader. Referência para testes e para o caminho sem GPU.
pub fn yuv_para_rgb(y: u8, cb: u8, cr: u8, matriz: Matriz, faixa: Faixa) -> [u8; 3] {
    let c = coeficientes(matriz, faixa);
    let v = [
        f32::from(y) / 255.0 - c[3][0],
        f32::from(cb) / 255.0 - c[3][1],
        f32::from(cr) / 255.0 - c[3][2],
    ];
    let canal = |l: [f32; 4]| {
        let x = l[0] * v[0] + l[1] * v[1] + l[2] * v[2];
        (x.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [canal(c[0]), canal(c[1]), canal(c[2])]
}

/// Pipelines e layout, um por device.
#[derive(Debug)]
pub struct Conversor {
    layout: wgpu::BindGroupLayout,
    nv12: wgpu::RenderPipeline,
    i420: wgpu::RenderPipeline,
    amostra: wgpu::Sampler,
}

impl Conversor {
    pub fn new(device: &wgpu::Device) -> Conversor {
        let shader = device.create_shader_module(wgpu::include_wgsl!("yuv.wgsl"));
        let textura = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("taildesk-yuv"),
            entries: &[
                textura(0),
                textura(1),
                textura(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("taildesk-yuv"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entrada: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entrada),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entrada),
                    compilation_options: Default::default(),
                    targets: &[Some(FORMATO_SAIDA.into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let amostra = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("taildesk-yuv"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Conversor {
            nv12: pipeline("fs_nv12"),
            i420: pipeline("fs_i420"),
            layout,
            amostra,
        }
    }
}

/// Um plano de um quadro: bytes e o passo entre linhas (pode ser maior que a largura).
#[derive(Clone, Copy, Debug)]
pub struct Plano<'a> {
    pub dados: &'a [u8],
    pub passo: u32,
}

/// Erro ao enviar um quadro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErroQuadro {
    /// Quantidade de planos não bate com o formato.
    Planos,
    /// Um plano é menor que `passo * linhas` ou o passo é menor que a largura.
    Tamanho,
}

impl std::fmt::Display for ErroQuadro {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ErroQuadro::Planos => "quantidade de planos errada para o formato",
            ErroQuadro::Tamanho => "plano menor que o tamanho do quadro",
        })
    }
}

impl std::error::Error for ErroQuadro {}

/// Vídeo de um monitor remoto: texturas dos planos e a textura RGBA de saída.
#[derive(Debug)]
pub struct Superficie {
    formato: Formato,
    largura: u32,
    altura: u32,
    planos: Vec<wgpu::Texture>,
    saida: wgpu::Texture,
    vista_saida: wgpu::TextureView,
    grupo: wgpu::BindGroup,
}

impl Superficie {
    pub fn new(
        device: &wgpu::Device,
        conversor: &Conversor,
        formato: Formato,
        largura: u32,
        altura: u32,
        matriz: Matriz,
        faixa: Faixa,
    ) -> Superficie {
        let (cl, ca) = (largura.div_ceil(2), altura.div_ceil(2));
        let textura = |nome, l, a, fmt, uso| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(nome),
                size: wgpu::Extent3d {
                    width: l,
                    height: a,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: fmt,
                usage: uso,
                view_formats: &[],
            })
        };
        let entrada = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let planos = match formato {
            Formato::Nv12 => vec![
                textura("y", largura, altura, wgpu::TextureFormat::R8Unorm, entrada),
                textura("uv", cl, ca, wgpu::TextureFormat::Rg8Unorm, entrada),
            ],
            Formato::I420 => vec![
                textura("y", largura, altura, wgpu::TextureFormat::R8Unorm, entrada),
                textura("u", cl, ca, wgpu::TextureFormat::R8Unorm, entrada),
                textura("v", cl, ca, wgpu::TextureFormat::R8Unorm, entrada),
            ],
        };
        // O Slint exige as duas: amostrar e (no renderer dele) ser alvo.
        let saida = textura(
            "rgba",
            largura,
            altura,
            FORMATO_SAIDA,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
        );
        let cores = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cores"),
            contents: &bytes_f32(&coeficientes(matriz, faixa)),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let vistas: Vec<_> = planos
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        // NV12 não tem terceiro plano; o shader não o lê, mas o layout pede.
        let v = vistas.get(2).unwrap_or(&vistas[1]);
        let grupo = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("taildesk-yuv"),
            layout: &conversor.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&vistas[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&vistas[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(v),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&conversor.amostra),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: cores.as_entire_binding(),
                },
            ],
        });
        Superficie {
            formato,
            largura,
            altura,
            vista_saida: saida.create_view(&Default::default()),
            planos,
            saida,
            grupo,
        }
    }

    pub fn tamanho(&self) -> (u32, u32) {
        (self.largura, self.altura)
    }

    pub fn formato(&self) -> Formato {
        self.formato
    }

    /// Textura RGBA com o último quadro. Clonar é barato (contagem de referência);
    /// `slint::Image::try_from(textura)` mostra na janela.
    pub fn textura(&self) -> &wgpu::Texture {
        &self.saida
    }

    /// Sobe os planos e converte para RGBA. Nada aloca por quadro além do que o `wgpu` faz na
    /// fila; chame da thread que tem o device (a da UI, com o Slint).
    pub fn quadro(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        conversor: &Conversor,
        planos: &[Plano<'_>],
    ) -> Result<(), ErroQuadro> {
        if planos.len() != self.planos.len() {
            return Err(ErroQuadro::Planos);
        }
        for (textura, plano) in self.planos.iter().zip(planos) {
            let t = textura.size();
            let bytes_px = textura.format().block_copy_size(None).unwrap_or(1);
            let linha = t.width * bytes_px;
            let necessario = u64::from(plano.passo) * u64::from(t.height - 1) + u64::from(linha);
            if plano.passo < linha || (plano.dados.len() as u64) < necessario {
                return Err(ErroQuadro::Tamanho);
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: textura,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                plano.dados,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(plano.passo),
                    rows_per_image: Some(t.height),
                },
                t,
            );
        }
        let mut cmd = device.create_command_encoder(&Default::default());
        {
            let mut passo = cmd.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("taildesk-yuv"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.vista_saida,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            passo.set_pipeline(match self.formato {
                Formato::Nv12 => &conversor.nv12,
                Formato::I420 => &conversor.i420,
            });
            passo.set_bind_group(0, &self.grupo, &[]);
            passo.draw(0..3, 0..1);
        }
        queue.submit([cmd.finish()]);
        Ok(())
    }
}

fn bytes_f32(m: &[[f32; 4]; 4]) -> Vec<u8> {
    m.iter().flatten().flat_map(|f| f.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests;
