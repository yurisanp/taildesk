//! Decoder por Media Foundation (Windows): o decoder do sistema para o codec (H.264, HEVC ou AV1)
//! com um device D3D11, então a decodificação roda na GPU (DXVA). O quadro sai numa textura NV12,
//! é copiado para uma textura de staging e lido pela CPU, como nos outros decoders.
//!
//! Sem D3D11 (driver ruim, sessão remota) o mesmo decoder roda em software e [`DecoderMf::gpu`]
//! fica `None`.

use std::mem::ManuallyDrop;
use std::sync::OnceLock;

use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::core::{GUID, Interface};

use crate::{CodecVideo, ErroCodec, Faixa, FormatoYuv, Matriz, PlanoYuv, QuadroYuv, VideoDecoder};

fn erro(o_que: &str, e: impl std::fmt::Display) -> ErroCodec {
    ErroCodec::Backend(format!("mf: {o_que}: {e}"))
}

/// MTA para o processo inteiro e Media Foundation iniciado, uma vez.
fn iniciar() -> Result<(), ErroCodec> {
    static INICIO: OnceLock<Result<(), String>> = OnceLock::new();
    INICIO
        .get_or_init(|| {
            // SAFETY: as duas chamadas só preparam o COM/MF do processo e não são desfeitas:
            // com o MTA mantido, qualquer thread (a do decode) usa os objetos MF.
            unsafe {
                CoIncrementMTAUsage().map_err(|e| e.to_string())?;
                MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| e.to_string())
            }
        })
        .clone()
        .map_err(|e| erro("MFStartup", e))
}

fn subtipo(codec: CodecVideo) -> GUID {
    match codec {
        CodecVideo::H264 => MFVideoFormat_H264,
        CodecVideo::Hevc => MFVideoFormat_HEVC,
        CodecVideo::Av1 => MFVideoFormat_AV1,
    }
}

/// Decoders do sistema para o codec, ativados, na ordem do MF. Inclui os da Microsoft Store
/// (as extensões de HEVC e AV1 vêm de lá).
fn ativar_decoders(codec: CodecVideo) -> Result<Vec<IMFTransform>, ErroCodec> {
    let entrada = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtipo(codec),
    };
    let mut lista: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut n = 0u32;
    // SAFETY: `lista` recebe um array de `n` ativadores alocado pelo MF, liberado abaixo com
    // `CoTaskMemFree` depois de soltar cada elemento.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER | MFT_ENUM_FLAG_UNTRUSTED_STOREMFT,
            Some(&entrada),
            None,
            &mut lista,
            &mut n,
        )
        .map_err(|e| erro("MFTEnumEx", e))?;
        // Sem decoder registrado o MF devolve zero itens e, às vezes, a lista nula.
        if lista.is_null() || n == 0 {
            windows::Win32::System::Com::CoTaskMemFree(Some(lista.cast()));
            return Err(erro("MFTEnumEx", format!("sem decoder para {codec:?}")));
        }
        let ativadores = std::slice::from_raw_parts_mut(lista, n as usize);
        let mut mfts = Vec::new();
        for a in ativadores.iter_mut() {
            if let Some(a) = a.as_ref()
                && let Ok(t) = a.ActivateObject::<IMFTransform>()
            {
                mfts.push(t);
            }
            drop(a.take());
        }
        windows::Win32::System::Com::CoTaskMemFree(Some(lista.cast()));
        Ok(mfts)
    }
}

/// Device D3D11 com suporte a vídeo na primeira GPU e o nome dela.
fn device() -> Result<(ID3D11Device, ID3D11DeviceContext, String), ErroCodec> {
    // SAFETY: chamadas COM/D3D11 com saídas válidas; os objetos ficam com contagem própria.
    unsafe {
        let fabrica: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| erro("DXGI", e))?;
        let adaptador: IDXGIAdapter1 = fabrica.EnumAdapters1(0).map_err(|e| erro("DXGI", e))?;
        let desc = adaptador.GetDesc1().map_err(|e| erro("DXGI", e))?;
        let fim = desc.Description.iter().position(|&c| c == 0).unwrap_or(128);
        let nome = String::from_utf16_lossy(&desc.Description[..fim]);
        let (mut dev, mut ctx) = (None, None);
        D3D11CreateDevice(
            &adaptador,
            D3D_DRIVER_TYPE_UNKNOWN,
            Default::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            Some(&mut ctx),
        )
        .map_err(|e| erro("D3D11CreateDevice", e))?;
        let (dev, ctx) = dev
            .zip(ctx)
            .ok_or_else(|| erro("D3D11CreateDevice", "sem device"))?;
        // O decoder usa o device de outra thread interna.
        if let Ok(mt) = dev.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
        Ok((dev, ctx, nome.trim().to_owned()))
    }
}

/// O que a GPU precisa para ler o quadro decodificado.
struct Gpu {
    _gerente: IMFDXGIDeviceManager,
    device: ID3D11Device,
    contexto: ID3D11DeviceContext,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    nome: String,
}

/// Decoder Media Foundation de um codec.
pub struct DecoderMf {
    mft: IMFTransform,
    gpu: Option<Gpu>,
    codec: CodecVideo,
    /// O decoder aloca as amostras de saída (com D3D11) ou somos nós (software).
    fornece_amostras: bool,
    tamanho_buffer: u32,
    /// Tamanho codificado (com o alinhamento) e o visível.
    codificado: (u32, u32),
    visivel: (u32, u32),
    y: Vec<u8>,
    uv: Vec<u8>,
    tempo: i64,
}

// SAFETY: o processo inteiro está no MTA (`iniciar`), onde os objetos MF e D3D11 podem ser usados
// de qualquer thread; o decoder é `&mut self` em tudo, então uma thread de cada vez.
unsafe impl Send for DecoderMf {}

impl std::fmt::Debug for DecoderMf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecoderMf")
            .field("codec", &self.codec)
            .field("gpu", &self.gpu.as_ref().map(|g| &g.nome))
            .finish_non_exhaustive()
    }
}

impl DecoderMf {
    /// Abre o decoder do sistema para `codec`, na GPU se der.
    pub fn novo(codec: CodecVideo) -> Result<Self, ErroCodec> {
        iniciar()?;
        let mut erros = Vec::new();
        for (i, mft) in ativar_decoders(codec)?.into_iter().enumerate() {
            match Self::configurar(mft, codec) {
                Ok(d) => return Ok(d),
                Err(e) => erros.push(format!("decoder {i}: {e}")),
            }
        }
        Err(erro("decoder", erros.join("; ")))
    }

    fn configurar(mft: IMFTransform, codec: CodecVideo) -> Result<Self, ErroCodec> {
        // SAFETY: chamadas COM no MFT recém-ativado; os tipos de mídia são criados aqui e o
        // gerente D3D fica guardado enquanto o decoder existir.
        unsafe {
            let atributos = mft.GetAttributes().map_err(|e| erro("GetAttributes", e))?;
            let _ = atributos.SetUINT32(&MF_LOW_LATENCY, 1);
            let d3d11 = atributos.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0;
            let gpu = if d3d11 {
                Self::ligar_gpu(&mft).ok()
            } else {
                None
            };

            // Tamanho e taxa de partida: alguns decoders (HEVC, AV1) recusam o tipo sem eles; o
            // tamanho real vem do stream (MF_E_TRANSFORM_STREAM_CHANGE).
            let tipo = MFCreateMediaType().map_err(|e| erro("MFCreateMediaType", e))?;
            tipo.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| erro("tipo", e))?;
            tipo.SetGUID(&MF_MT_SUBTYPE, &subtipo(codec))
                .map_err(|e| erro("tipo", e))?;
            let _ = tipo.SetUINT64(&MF_MT_FRAME_SIZE, (1920u64 << 32) | 1080);
            let _ = tipo.SetUINT64(&MF_MT_FRAME_RATE, (60u64 << 32) | 1);
            let _ = tipo.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32);
            mft.SetInputType(0, &tipo, 0)
                .map_err(|e| erro("SetInputType", e))?;
            let mut d = DecoderMf {
                mft,
                gpu,
                codec,
                fornece_amostras: false,
                tamanho_buffer: 0,
                codificado: (0, 0),
                visivel: (0, 0),
                y: Vec::new(),
                uv: Vec::new(),
                tempo: 0,
            };
            d.tipo_de_saida()?;
            let _ = d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            let _ = d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
            Ok(d)
        }
    }

    /// Entrega um device D3D11 ao decoder (decodificação na GPU).
    ///
    /// # Safety
    /// `mft` é um decoder MF válido, ainda sem tipos de mídia.
    unsafe fn ligar_gpu(mft: &IMFTransform) -> Result<Gpu, ErroCodec> {
        let (device, contexto, nome) = device()?;
        let mut token = 0u32;
        let mut gerente = None;
        // SAFETY: saídas válidas; o gerente recebe o device e fica vivo na `Gpu`, que vive com
        // o decoder.
        unsafe {
            MFCreateDXGIDeviceManager(&mut token, &mut gerente)
                .map_err(|e| erro("MFCreateDXGIDeviceManager", e))?;
            let gerente = gerente.ok_or_else(|| erro("MFCreateDXGIDeviceManager", "vazio"))?;
            gerente
                .ResetDevice(&device, token)
                .map_err(|e| erro("ResetDevice", e))?;
            mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, gerente.as_raw() as usize)
                .map_err(|e| erro("SET_D3D_MANAGER", e))?;
            Ok(Gpu {
                _gerente: gerente,
                device,
                contexto,
                staging: None,
                nome,
            })
        }
    }

    /// Escolhe NV12 na saída e lê o tamanho (também depois de `MF_E_TRANSFORM_STREAM_CHANGE`).
    fn tipo_de_saida(&mut self) -> Result<(), ErroCodec> {
        // SAFETY: chamadas COM no MFT deste decoder; o blob da abertura é lido para uma
        // `MFVideoArea` com o tamanho dela.
        unsafe {
            for i in 0.. {
                let tipo = match self.mft.GetOutputAvailableType(0, i) {
                    Ok(t) => t,
                    Err(e) if e.code() == MF_E_TRANSFORM_TYPE_NOT_SET => return Ok(()),
                    Err(e) => return Err(erro("GetOutputAvailableType", e)),
                };
                if tipo.GetGUID(&MF_MT_SUBTYPE).ok() != Some(MFVideoFormat_NV12) {
                    continue;
                }
                self.mft
                    .SetOutputType(0, &tipo, 0)
                    .map_err(|e| erro("SetOutputType", e))?;
                if let Ok(t) = tipo.GetUINT64(&MF_MT_FRAME_SIZE) {
                    self.codificado = ((t >> 32) as u32, t as u32);
                    self.visivel = self.codificado;
                }
                let mut area = MFVideoArea::default();
                let bytes = std::slice::from_raw_parts_mut(
                    (&mut area as *mut MFVideoArea).cast::<u8>(),
                    size_of::<MFVideoArea>(),
                );
                if tipo
                    .GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, bytes, None)
                    .is_ok()
                    && area.Area.cx > 0
                    && area.Area.cy > 0
                {
                    self.visivel = (area.Area.cx as u32, area.Area.cy as u32);
                }
                let info = self
                    .mft
                    .GetOutputStreamInfo(0)
                    .map_err(|e| erro("GetOutputStreamInfo", e))?;
                self.fornece_amostras = info.dwFlags
                    & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0
                        | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32
                    != 0;
                self.tamanho_buffer = info.cbSize;
                return Ok(());
            }
        }
        Ok(())
    }

    /// Copia o NV12 visível da amostra para `y`/`uv`.
    fn ler(&mut self, amostra: &IMFSample) -> Result<(), ErroCodec> {
        let (l, a) = (self.visivel.0 as usize & !1, self.visivel.1 as usize & !1);
        self.y.resize(l * a, 0);
        self.uv.resize(l * a / 2, 0);
        // SAFETY: o buffer vem da amostra que o decoder acabou de entregar; a textura ou a
        // memória ficam mapeadas só entre lock/map e unlock/unmap, e cada linha lida está
        // dentro do passo e da altura que a API devolveu.
        unsafe {
            let buffer = amostra
                .GetBufferByIndex(0)
                .map_err(|e| erro("GetBufferByIndex", e))?;
            if let (Some(gpu), Ok(dxgi)) = (self.gpu.as_mut(), buffer.cast::<IMFDXGIBuffer>()) {
                let mut ptr = std::ptr::null_mut();
                dxgi.GetResource(&ID3D11Texture2D::IID, &mut ptr)
                    .map_err(|e| erro("GetResource", e))?;
                let textura = ID3D11Texture2D::from_raw(ptr);
                let indice = dxgi.GetSubresourceIndex().unwrap_or(0);
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                textura.GetDesc(&mut desc);
                if gpu
                    .staging
                    .as_ref()
                    .is_none_or(|(_, w, h)| (*w, *h) != (desc.Width, desc.Height))
                {
                    let st = D3D11_TEXTURE2D_DESC {
                        Width: desc.Width,
                        Height: desc.Height,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_NV12,
                        SampleDesc: desc.SampleDesc,
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        MiscFlags: 0,
                    };
                    let mut t = None;
                    gpu.device
                        .CreateTexture2D(&st, None, Some(&mut t))
                        .map_err(|e| erro("CreateTexture2D", e))?;
                    gpu.staging = t.map(|t| (t, desc.Width, desc.Height));
                }
                let (staging, _, altura_tex) = gpu
                    .staging
                    .as_ref()
                    .ok_or_else(|| erro("staging", "sem textura"))?;
                gpu.contexto
                    .CopySubresourceRegion(staging, 0, 0, 0, 0, &textura, indice, None);
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                gpu.contexto
                    .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
                    .map_err(|e| erro("Map", e))?;
                let passo = m.RowPitch as usize;
                let base = m.pData.cast::<u8>().cast_const();
                copiar_nv12(
                    base,
                    passo,
                    *altura_tex as usize,
                    l,
                    a,
                    &mut self.y,
                    &mut self.uv,
                );
                gpu.contexto.Unmap(staging, 0);
            } else {
                let b2 = buffer
                    .cast::<IMF2DBuffer>()
                    .map_err(|e| erro("IMF2DBuffer", e))?;
                let (mut p, mut passo) = (std::ptr::null_mut(), 0i32);
                b2.Lock2D(&mut p, &mut passo)
                    .map_err(|e| erro("Lock2D", e))?;
                copiar_nv12(
                    p,
                    passo.unsigned_abs() as usize,
                    self.codificado.1 as usize,
                    l,
                    a,
                    &mut self.y,
                    &mut self.uv,
                );
                let _ = b2.Unlock2D();
            }
        }
        Ok(())
    }
}

/// NV12 com passo `passo` e `altura_plano` linhas no plano Y (o UV vem logo depois) para os
/// buffers compactos de `l`×`a`.
///
/// # Safety
/// `base` aponta para `altura_plano * passo * 3 / 2` bytes legíveis, com `l <= passo` e
/// `a <= altura_plano`.
unsafe fn copiar_nv12(
    base: *const u8,
    passo: usize,
    altura_plano: usize,
    l: usize,
    a: usize,
    y: &mut [u8],
    uv: &mut [u8],
) {
    for (i, linha) in y.chunks_exact_mut(l).take(a).enumerate() {
        // SAFETY: linha `i` < `a` <= `altura_plano`, `l` bytes dentro do passo.
        linha.copy_from_slice(unsafe { std::slice::from_raw_parts(base.add(i * passo), l) });
    }
    // SAFETY: o plano UV começa depois de `altura_plano` linhas.
    let base_uv = unsafe { base.add(altura_plano * passo) };
    for (i, linha) in uv.chunks_exact_mut(l).take(a / 2).enumerate() {
        // SAFETY: linha `i` < `a / 2` do plano UV, `l` bytes dentro do passo.
        linha.copy_from_slice(unsafe { std::slice::from_raw_parts(base_uv.add(i * passo), l) });
    }
}

impl VideoDecoder for DecoderMf {
    fn nome(&self) -> &'static str {
        if self.gpu.is_some() { "mf-d3d11" } else { "mf" }
    }

    fn gpu(&self) -> Option<&str> {
        self.gpu.as_ref().map(|g| g.nome.as_str())
    }

    fn decodificar(
        &mut self,
        access_unit: &[u8],
        ao_quadro: &mut dyn FnMut(&QuadroYuv<'_>),
    ) -> Result<bool, ErroCodec> {
        // SAFETY: a amostra e o buffer são criados aqui; o buffer fica travado só durante a
        // cópia de `access_unit`, que cabe nele (alocado com esse tamanho). As amostras de
        // saída voltam com contagem própria e são soltas no fim de cada volta.
        unsafe {
            let buffer = MFCreateMemoryBuffer(access_unit.len() as u32)
                .map_err(|e| erro("MFCreateMemoryBuffer", e))?;
            let mut p = std::ptr::null_mut();
            buffer
                .Lock(&mut p, None, None)
                .map_err(|e| erro("Lock", e))?;
            std::ptr::copy_nonoverlapping(access_unit.as_ptr(), p, access_unit.len());
            buffer.Unlock().map_err(|e| erro("Unlock", e))?;
            buffer
                .SetCurrentLength(access_unit.len() as u32)
                .map_err(|e| erro("SetCurrentLength", e))?;
            let amostra = MFCreateSample().map_err(|e| erro("MFCreateSample", e))?;
            amostra
                .AddBuffer(&buffer)
                .map_err(|e| erro("AddBuffer", e))?;
            amostra
                .SetSampleTime(self.tempo)
                .map_err(|e| erro("SetSampleTime", e))?;
            self.tempo += 166_667;

            let mut saiu = false;
            let mut entrou = false;
            for _ in 0..8 {
                if !entrou {
                    match self.mft.ProcessInput(0, &amostra, 0) {
                        Ok(()) => entrou = true,
                        Err(e) if e.code() == MF_E_NOTACCEPTING => {}
                        Err(e) => return Err(erro("ProcessInput", e)),
                    }
                }
                // Tira tudo o que estiver pronto.
                loop {
                    let nossa = if self.fornece_amostras {
                        None
                    } else {
                        let b = MFCreateMemoryBuffer(self.tamanho_buffer.max(1))
                            .map_err(|e| erro("MFCreateMemoryBuffer", e))?;
                        let s = MFCreateSample().map_err(|e| erro("MFCreateSample", e))?;
                        s.AddBuffer(&b).map_err(|e| erro("AddBuffer", e))?;
                        Some(s)
                    };
                    let mut saida = [MFT_OUTPUT_DATA_BUFFER {
                        dwStreamID: 0,
                        pSample: ManuallyDrop::new(nossa),
                        dwStatus: 0,
                        pEvents: ManuallyDrop::new(None),
                    }];
                    let mut status = 0u32;
                    let r = self.mft.ProcessOutput(0, &mut saida, &mut status);
                    let amostra_saida = ManuallyDrop::take(&mut saida[0].pSample);
                    drop(ManuallyDrop::take(&mut saida[0].pEvents));
                    match r {
                        Ok(()) => {
                            if let Some(s) = amostra_saida {
                                self.ler(&s)?;
                                saiu = true;
                                let (l, a) = (self.visivel.0 & !1, self.visivel.1 & !1);
                                ao_quadro(&QuadroYuv {
                                    largura: l,
                                    altura: a,
                                    formato: FormatoYuv::Nv12,
                                    planos: [
                                        PlanoYuv {
                                            dados: &self.y,
                                            passo: l,
                                        },
                                        PlanoYuv {
                                            dados: &self.uv,
                                            passo: l,
                                        },
                                        PlanoYuv {
                                            dados: &[],
                                            passo: 0,
                                        },
                                    ],
                                    // Os encoders do TailDesk marcam BT.709 limitado.
                                    matriz: Matriz::Bt709,
                                    faixa: Faixa::Limitada,
                                });
                            }
                        }
                        Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
                        Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                            self.tipo_de_saida()?;
                        }
                        Err(e) => return Err(erro("ProcessOutput", e)),
                    }
                }
                if entrou {
                    break;
                }
            }
            Ok(saiu)
        }
    }
}

/// GPU em que o Media Foundation decodifica `codec` (`Ok`), ou por que não decodifica em
/// hardware (`Err`: sem decoder, falhou ao configurar ou só em software). Testado uma vez por
/// codec (abre um decoder).
pub fn sondar(codec: CodecVideo) -> Result<String, String> {
    static CACHE: std::sync::Mutex<Vec<(CodecVideo, Result<String, String>)>> =
        std::sync::Mutex::new(Vec::new());
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, r)) = cache.iter().find(|(c, _)| *c == codec) {
        return r.clone();
    }
    let r = match DecoderMf::novo(codec) {
        Ok(d) => d
            .gpu()
            .map(str::to_owned)
            .ok_or_else(|| "decoder só em software (sem D3D11)".to_string()),
        Err(e) => Err(e.to_string()),
    };
    cache.push((codec, r.clone()));
    r
}

/// GPU em que o Media Foundation decodifica `codec` em hardware (ver [`sondar`]).
pub fn gpu_do_decoder(codec: CodecVideo) -> Option<String> {
    sondar(codec).ok()
}
