//! Telas renderizadas sem GPU nem janela (software renderer do Slint): os testes de tela, a
//! conferência visual durante o desenvolvimento e as capturas da loja no CI
//! (`examples/capturas.rs`, `android-detalhado.md` §8). Só fora do Android.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{PhysicalSize, PlatformError};

struct PlataformaCaptura {
    janela: Rc<MinimalSoftwareWindow>,
    inicio: Instant,
}

impl Platform for PlataformaCaptura {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.janela.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.inicio.elapsed()
    }
}

/// Liga a plataforma de captura (uma vez por thread de teste) e devolve a janela.
pub fn iniciar() -> Rc<MinimalSoftwareWindow> {
    thread_local! {
        static JANELA: std::cell::OnceCell<Rc<MinimalSoftwareWindow>> = const { std::cell::OnceCell::new() };
    }
    JANELA.with(|j| {
        j.get_or_init(|| {
            let janela = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            slint::platform::set_platform(Box::new(PlataformaCaptura {
                janela: janela.clone(),
                inicio: Instant::now(),
            }))
            .expect("plataforma de captura");
            janela
        })
        .clone()
    })
}

/// Tamanho físico e escala (pixels físicos por pixel lógico).
pub fn dimensionar(janela: &MinimalSoftwareWindow, largura: u32, altura: u32, escala: f32) {
    janela.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: escala,
    });
    janela.set_size(PhysicalSize::new(largura, altura));
}

/// Desenha e devolve RGBA (`largura * altura * 4` bytes).
pub fn renderizar(janela: &MinimalSoftwareWindow) -> (u32, u32, Vec<u8>) {
    slint::platform::update_timers_and_animations();
    let tamanho = janela.size();
    let (w, h) = (tamanho.width as usize, tamanho.height as usize);
    let mut buf = vec![PremultipliedRgbaColor::default(); w * h];
    janela.request_redraw();
    janela.draw_if_needed(|r| {
        r.render(&mut buf, w);
    });
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in buf {
        // O fundo da janela é opaco: sem divisão por alfa.
        rgba.extend_from_slice(&[p.red, p.green, p.blue, 255]);
    }
    (tamanho.width, tamanho.height, rgba)
}
