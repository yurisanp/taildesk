//! Spike da fase 2 (`10-plano-windows-linux.md`, riscos): duas janelas Slint, cada uma com um
//! vídeo NV12 1920×1080 a 60 fps convertido no `wgpu`, todas no mesmo `wgpu::Device`.
//!
//! ```sh
//! cargo run --release -p taildesk-render --example duas_janelas
//! ```
//!
//! Cada janela mostra os quadros por segundo que o Slint desenhou de fato. Passa se as duas
//! ficam perto de 60 com o vídeo andando liso, e se fechar uma não derruba a outra.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::wgpu_30::{WGPUConfiguration, wgpu};
use slint::{CloseRequestResponse, ComponentHandle, Image, Timer, TimerMode};
use taildesk_render::{Conversor, Faixa, Formato, Matriz, Plano, Superficie};

slint::slint! {
    export component JanelaVideo inherits Window {
        in property <image> quadro;
        in property <string> info;
        title: "TailDesk spike: " + info;
        preferred-width: 960px;
        preferred-height: 540px;
        background: black;
        Image {
            source: quadro;
            width: 100%;
            height: 100%;
            image-fit: contain;
        }
        Text {
            x: 12px;
            y: 12px;
            text: info;
            color: white;
            font-size: 20px;
        }
    }
}

const L: u32 = 1920;
const A: u32 = 1080;

/// Planos com o dobro da largura: andar o início da fatia faz o vídeo correr sem gerar nada
/// por quadro.
struct Padrao {
    y: Vec<u8>,
    uv: Vec<u8>,
}

impl Padrao {
    fn new(tom: u8) -> Padrao {
        let passo = 2 * L;
        let mut y = vec![0u8; (passo * A) as usize];
        let mut uv = vec![0u8; (passo * A / 2) as usize];
        for py in 0..A {
            for px in 0..passo {
                // Barras com texto fino (linhas de 1 px) para ver a nitidez da escala.
                let barra = ((px / 120) % 8) as u8;
                let fio = if px % 240 < 2 || py % 60 < 1 { 200 } else { 0 };
                y[(py * passo + px) as usize] = 16u8.saturating_add(barra * 26).max(fio);
            }
        }
        for cy in 0..A / 2 {
            for cx in 0..L {
                let i = (cy * passo + cx * 2) as usize;
                uv[i] = tom.wrapping_add(((cx / 60) as u8).wrapping_mul(9));
                uv[i + 1] = 255 - tom.wrapping_add(((cy / 30) as u8).wrapping_mul(7));
            }
        }
        Padrao { y, uv }
    }

    fn planos(&self, quadro: u32) -> [Plano<'_>; 2] {
        let deslocamento = ((quadro * 8) % L) & !1;
        [
            Plano {
                dados: &self.y[deslocamento as usize..],
                passo: 2 * L,
            },
            Plano {
                dados: &self.uv[deslocamento as usize..],
                passo: 2 * L,
            },
        ]
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Um device só, entregue ao Slint: as texturas valem em qualquer janela.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
    eprintln!("adaptador: {:?}", adapter.get_info());
    slint::BackendSelector::new()
        .require_wgpu_30(WGPUConfiguration::Manual {
            instance,
            adapter,
            device: device.clone(),
            queue: queue.clone(),
        })
        .select()?;

    let conversor = Rc::new(Conversor::new(&device));
    let mut janelas = Vec::new();
    let abertas = Rc::new(Cell::new(0usize));
    for (n, tom) in [(1, 60u8), (2, 170u8)] {
        let janela = JanelaVideo::new()?;
        let superficie = Superficie::new(
            &device,
            &conversor,
            Formato::Nv12,
            L,
            A,
            Matriz::Bt709,
            Faixa::Limitada,
        );
        let padrao = Padrao::new(tom);
        let desenhados = Rc::new(Cell::new(0u32));
        {
            let desenhados = desenhados.clone();
            janela
                .window()
                .set_rendering_notifier(move |estado, _| {
                    if matches!(estado, slint::RenderingState::AfterRendering) {
                        desenhados.set(desenhados.get() + 1);
                    }
                })
                .map_err(|e| format!("{e:?}"))?;
        }
        let fraca = janela.as_weak();
        let (device, queue, conversor) = (device.clone(), queue.clone(), conversor.clone());
        let mut quadro = 0u32;
        let mut desde = Instant::now();
        let timer = Rc::new(Timer::default());
        timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
            let Some(janela) = fraca.upgrade() else {
                return;
            };
            quadro += 1;
            if let Err(e) = superficie.quadro(&queue, &device, &conversor, &padrao.planos(quadro)) {
                eprintln!("janela {n}: {e}");
                return;
            }
            if let Ok(imagem) = Image::try_from(superficie.textura().clone()) {
                janela.set_quadro(imagem);
            }
            if desde.elapsed() >= Duration::from_secs(1) {
                let fps = f64::from(desenhados.replace(0)) / desde.elapsed().as_secs_f64();
                janela.set_info(format!("janela {n}: {fps:.0} fps").into());
                desde = Instant::now();
            }
        });
        // Fechar a janela para o vídeo dela; fechar a última encerra o programa.
        {
            let (timer, abertas) = (timer.clone(), abertas.clone());
            janela.window().on_close_requested(move || {
                timer.stop();
                abertas.set(abertas.get() - 1);
                if abertas.get() == 0 {
                    let _ = slint::quit_event_loop();
                }
                CloseRequestResponse::HideWindow
            });
        }
        janela.show()?;
        abertas.set(abertas.get() + 1);
        janelas.push((janela, timer));
    }
    slint::run_event_loop_until_quit()?;
    Ok(())
}
