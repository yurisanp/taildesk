//! Vídeo no controlador: recepção e decode por monitor (`taildesk_media::video`) e a imagem na
//! janela da sessão.
//!
//! Com GPU, o app escolhe o renderer `wgpu` do Slint com um device nosso ([`Gpu`]) e cada quadro
//! vira uma textura RGBA no `taildesk_render` (`slint::Image::try_from(wgpu::Texture)`), sem
//! passar pela CPU. Sem GPU, o renderer padrão do Slint e a conversão I420 → RGBA na CPU.
//!
//! O decode roda na thread dele e deixa o quadro mais novo numa [`Caixa`] (latest-wins); a thread
//! da interface só é acordada quando a caixa passa de vazia a cheia.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use slint::wgpu_30::{WGPUConfiguration, wgpu};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use taildesk_codec::{ErroCodec, Faixa, FormatoYuv, Matriz, QuadroYuv, VideoDecoder};
use taildesk_media::video::RecepcaoVideo;
use taildesk_render::{Conversor, Plano, Superficie};

/// Device do `wgpu` que o Slint também usa: as texturas valem em qualquer janela.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    conversor: Conversor,
}

thread_local! {
    /// `None` = sem GPU (renderer padrão e conversão na CPU).
    static GPU: RefCell<Option<Rc<Gpu>>> = const { RefCell::new(None) };
}

/// Pede um adaptador e devolve a configuração para o Slint. `None` se não houver GPU utilizável.
pub fn preparar_gpu() -> Option<WGPUConfiguration> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    let gpu = Gpu {
        conversor: Conversor::new(&device),
        device: device.clone(),
        queue: queue.clone(),
    };
    GPU.with(|g| *g.borrow_mut() = Some(Rc::new(gpu)));
    Some(WGPUConfiguration::Manual {
        instance,
        adapter,
        device,
        queue,
    })
}

/// O Slint não aceitou o renderer `wgpu`: as texturas não aparecem, então converte na CPU.
pub fn sem_gpu() {
    GPU.with(|g| g.borrow_mut().take());
}

/// Planos de um quadro decodificado, copiados do decoder.
#[derive(Debug, Default)]
struct Planos {
    dados: [Vec<u8>; 3],
    passos: [u32; 3],
    largura: u32,
    altura: u32,
    nv12: bool,
    bt601: bool,
    cheia: bool,
}

impl Planos {
    fn copiar(&mut self, q: &QuadroYuv<'_>) {
        for (i, p) in q.planos.iter().enumerate() {
            self.dados[i].clear();
            self.dados[i].extend_from_slice(p.dados);
            self.passos[i] = p.passo;
        }
        self.largura = q.largura;
        self.altura = q.altura;
        self.nv12 = q.formato == FormatoYuv::Nv12;
        self.bt601 = q.matriz == Matriz::Bt601;
        self.cheia = q.faixa == Faixa::Cheia;
    }

    /// Os planos para o render e quantos valem (2 em NV12, 3 em I420).
    fn planos(&self) -> ([Plano<'_>; 3], usize) {
        let p = |i: usize| Plano {
            dados: &self.dados[i],
            passo: self.passos[i],
        };
        ([p(0), p(1), p(2)], if self.nv12 { 2 } else { 3 })
    }
}

/// O quadro mais novo de uma sessão, entre a thread de decode e a da interface.
#[derive(Debug, Default)]
pub struct Caixa {
    quadro: Mutex<(Planos, bool)>,
    agendado: AtomicBool,
}

impl Caixa {
    fn travar(&self) -> MutexGuard<'_, (Planos, bool)> {
        self.quadro.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Thread de decode: guarda o quadro. `true` = a interface precisa ser acordada.
    pub fn guardar(&self, q: &QuadroYuv<'_>) -> bool {
        {
            let mut g = self.travar();
            g.0.copiar(q);
            g.1 = true;
        }
        !self.agendado.swap(true, Ordering::AcqRel)
    }
}

/// Contadores do vídeo de uma sessão no controlador, para o painel de diagnóstico.
#[derive(Debug, Default)]
pub struct Contadores {
    pub datagramas: AtomicU64,
    pub bytes: AtomicU64,
    pub decodificados: AtomicU64,
    /// Soma dos tempos de decode, em µs.
    pub decode_us: AtomicU64,
    pub erros_decode: AtomicU64,
    /// Quadros que se perderam e pediram quadro-chave ao host.
    pub pedidos_chave: AtomicU64,
    /// Quadros que a janela mostrou (os outros foram substituídos antes).
    pub exibidos: AtomicU64,
    /// Backend e GPU do decoder em uso.
    decoder: Mutex<(&'static str, String)>,
}

impl Contadores {
    pub fn decoder(&self) -> (&'static str, String) {
        self.decoder
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Decoder que conta quadros e tempo de decode em [`Contadores`].
pub struct DecoderMedido {
    dentro: Box<dyn VideoDecoder>,
    contadores: Arc<Contadores>,
}

impl DecoderMedido {
    pub fn novo(dentro: Box<dyn VideoDecoder>, contadores: Arc<Contadores>) -> Self {
        *contadores
            .decoder
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            (dentro.nome(), dentro.gpu().unwrap_or_default().to_owned());
        DecoderMedido { dentro, contadores }
    }
}

impl VideoDecoder for DecoderMedido {
    fn nome(&self) -> &'static str {
        self.dentro.nome()
    }

    fn gpu(&self) -> Option<&str> {
        self.dentro.gpu()
    }

    fn decodificar(
        &mut self,
        access_unit: &[u8],
        ao_quadro: &mut dyn FnMut(&QuadroYuv<'_>),
    ) -> Result<bool, ErroCodec> {
        let t = Instant::now();
        let r = self.dentro.decodificar(access_unit, ao_quadro);
        let c = &self.contadores;
        c.decode_us
            .fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
        match &r {
            Ok(true) => c.decodificados.fetch_add(1, Ordering::Relaxed),
            Ok(false) => 0,
            Err(_) => c.erros_decode.fetch_add(1, Ordering::Relaxed),
        };
        r
    }
}

/// Recepções de uma sessão por `stream_id` (a task dos datagramas e a interface mexem).
pub type Recepcoes = Arc<Mutex<HashMap<u8, (u32, RecepcaoVideo)>>>;

pub fn travar(r: &Recepcoes) -> MutexGuard<'_, HashMap<u8, (u32, RecepcaoVideo)>> {
    r.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lê os datagramas da conexão e entrega cada um à recepção do stream dele (byte 1 do header).
pub async fn ler_datagramas(
    conn: quinn::Connection,
    recepcoes: Recepcoes,
    contadores: Arc<Contadores>,
) {
    while let Ok(d) = conn.read_datagram().await {
        contadores.datagramas.fetch_add(1, Ordering::Relaxed);
        contadores
            .bytes
            .fetch_add(d.len() as u64, Ordering::Relaxed);
        let Some(&stream) = d.get(1) else { continue };
        if let Some((_, r)) = travar(&recepcoes).get_mut(&stream) {
            r.receber(&d);
        }
    }
}

/// Imagem da janela de uma sessão.
#[derive(Default)]
pub struct Tela {
    caixa: Arc<Caixa>,
    local: Planos,
    superficie: Option<Superficie>,
}

impl Tela {
    pub fn caixa(&self) -> Arc<Caixa> {
        self.caixa.clone()
    }

    /// Thread da interface: o quadro mais novo como imagem, se chegou um.
    pub fn proxima_imagem(&mut self) -> Option<Image> {
        self.caixa.agendado.store(false, Ordering::Release);
        {
            let mut g = self.caixa.travar();
            if !g.1 {
                return None;
            }
            g.1 = false;
            std::mem::swap(&mut g.0, &mut self.local);
        }
        let q = &self.local;
        if q.largura < 2 || q.altura < 2 {
            return None;
        }
        match GPU.with(|g| g.borrow().clone()) {
            Some(gpu) => self.na_gpu(&gpu),
            None => Some(na_cpu(q)),
        }
    }

    fn na_gpu(&mut self, gpu: &Gpu) -> Option<Image> {
        use taildesk_render::{Faixa, Formato, Matriz};
        let q = &self.local;
        let formato = if q.nv12 { Formato::Nv12 } else { Formato::I420 };
        let serve = self
            .superficie
            .as_ref()
            .is_some_and(|s| s.tamanho() == (q.largura, q.altura) && s.formato() == formato);
        if !serve {
            self.superficie = Some(Superficie::new(
                &gpu.device,
                &gpu.conversor,
                formato,
                q.largura,
                q.altura,
                if q.bt601 {
                    Matriz::Bt601
                } else {
                    Matriz::Bt709
                },
                if q.cheia {
                    Faixa::Cheia
                } else {
                    Faixa::Limitada
                },
            ));
        }
        let s = self.superficie.as_ref()?;
        let (planos, n) = q.planos();
        if let Err(e) = s.quadro(&gpu.queue, &gpu.device, &gpu.conversor, &planos[..n]) {
            eprintln!("taildesk: vídeo: {e}");
            return None;
        }
        Image::try_from(s.textura().clone()).ok()
    }
}

/// I420 (ou NV12) → RGBA na CPU, em ponto fixo (8 bits de fração).
fn na_cpu(q: &Planos) -> Image {
    let (l, a) = (q.largura, q.altura);
    // Coeficientes ×256: (fator de Y, V→R, U→G, V→G, U→B).
    let (ky, vr, ug, vg, ub) = match (q.bt601, q.cheia) {
        (false, false) => (298, 459, 55, 136, 541),
        (false, true) => (256, 403, 48, 120, 475),
        (true, false) => (298, 409, 100, 208, 516),
        (true, true) => (256, 359, 88, 183, 454),
    };
    let oy = if q.cheia { 0 } else { 16 };
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(l, a);
    let saida = buf.make_mut_slice();
    for y in 0..a as usize {
        let ly = &q.dados[0][y * q.passos[0] as usize..];
        for x in 0..l as usize {
            let (u, v) = if q.nv12 {
                let i = (y / 2) * q.passos[1] as usize + (x / 2) * 2;
                (q.dados[1][i], q.dados[1][i + 1])
            } else {
                (
                    q.dados[1][(y / 2) * q.passos[1] as usize + x / 2],
                    q.dados[2][(y / 2) * q.passos[2] as usize + x / 2],
                )
            };
            let c = ky * (i32::from(ly[x]) - oy);
            let (d, e) = (i32::from(u) - 128, i32::from(v) - 128);
            let px = |t: i32| ((t + 128) >> 8).clamp(0, 255) as u8;
            saida[y * l as usize + x] = Rgba8Pixel {
                r: px(c + vr * e),
                g: px(c - ug * d - vg * e),
                b: px(c + ub * d),
                a: 255,
            };
        }
    }
    Image::from_rgba8(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use taildesk_codec::PlanoYuv;

    #[test]
    fn cpu_bate_com_a_referencia_do_render() {
        // 4×2 I420 com valores variados.
        let y = [16u8, 80, 128, 235, 50, 100, 200, 30];
        let u = [90u8, 200];
        let v = [240u8, 60];
        let q = QuadroYuv {
            largura: 4,
            altura: 2,
            formato: FormatoYuv::I420,
            planos: [
                PlanoYuv {
                    dados: &y,
                    passo: 4,
                },
                PlanoYuv {
                    dados: &u,
                    passo: 2,
                },
                PlanoYuv {
                    dados: &v,
                    passo: 2,
                },
            ],
            matriz: Matriz::Bt709,
            faixa: Faixa::Limitada,
        };
        let caixa = Caixa::default();
        assert!(caixa.guardar(&q));
        // Já agendado: não acorda de novo até a interface pegar.
        assert!(!caixa.guardar(&q));
        let mut p = Planos::default();
        p.copiar(&q);
        let img = na_cpu(&p);
        let buf = img.to_rgba8().unwrap();
        for py in 0..2usize {
            for px in 0..4usize {
                let esperado = taildesk_render::yuv_para_rgb(
                    y[py * 4 + px],
                    u[px / 2],
                    v[px / 2],
                    taildesk_render::Matriz::Bt709,
                    taildesk_render::Faixa::Limitada,
                );
                let r = buf.as_slice()[py * 4 + px];
                for (a, b) in [r.r, r.g, r.b].into_iter().zip(esperado) {
                    assert!(a.abs_diff(b) <= 2, "({px},{py}): {a} vs {b}");
                }
            }
        }
    }
}
