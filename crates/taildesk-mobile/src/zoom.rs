//! Zoom e pan locais do vídeo e o mapa tela do celular → coordenada do monitor do host
//! (`mobile/arquitetura.md` §5.2, MF-41/42).
//!
//! A imagem do monitor cabe na área de vídeo com faixas pretas (como `Fit::Window`); a pinça
//! amplia em volta dos dedos e o arrasto (no toque direto) ou o cursor (no touchpad) movem a
//! janela. Nada disso vai ao host: só muda a transformação na GPU e as coordenadas que o toque
//! direto manda.

use taildesk_proto::control::limits::COORD_MAX;

/// Zoom máximo (5× já mostra texto de 8 pt de um 4K num celular).
pub const ZOOM_MAX: f32 = 5.0;

/// Retângulo em pixels da tela do celular.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Retangulo {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Transformação do vídeo de um monitor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vista {
    /// Área disponível para o vídeo.
    area: Retangulo,
    /// Tamanho do monitor do host em pixels.
    monitor: (u32, u32),
    escala: f32,
    /// Centro da janela visível, em fração do monitor (0..=1).
    cx: f32,
    cy: f32,
}

impl Vista {
    pub fn new(area: Retangulo, monitor: (u32, u32)) -> Vista {
        Vista {
            area,
            monitor: (monitor.0.max(1), monitor.1.max(1)),
            escala: 1.0,
            cx: 0.5,
            cy: 0.5,
        }
    }

    /// A tela girou ou o monitor mudou de tamanho: mantém zoom e centro (MF-49).
    pub fn redimensionar(&mut self, area: Retangulo, monitor: (u32, u32)) {
        self.area = area;
        self.monitor = (monitor.0.max(1), monitor.1.max(1));
        self.limitar();
    }

    pub fn escala(&self) -> f32 {
        self.escala
    }

    /// Escala que faz o monitor inteiro caber na área.
    fn caber(&self) -> f32 {
        (self.area.w / self.monitor.0 as f32).min(self.area.h / self.monitor.1 as f32)
    }

    /// Onde a imagem é desenhada (pode passar da área com zoom).
    pub fn imagem(&self) -> Retangulo {
        let s = self.caber() * self.escala;
        let (w, h) = (self.monitor.0 as f32 * s, self.monitor.1 as f32 * s);
        let x = self.area.x + self.area.w / 2.0 - self.cx * w;
        let y = self.area.y + self.area.h / 2.0 - self.cy * h;
        Retangulo { x, y, w, h }
    }

    /// Mantém a janela dentro do monitor (sem zoom, centralizado).
    fn limitar(&mut self) {
        self.escala = self.escala.clamp(1.0, ZOOM_MAX);
        let img = self.imagem();
        let meia_w = (self.area.w / img.w / 2.0).min(0.5);
        let meia_h = (self.area.h / img.h / 2.0).min(0.5);
        self.cx = self.cx.clamp(meia_w, 1.0 - meia_w);
        self.cy = self.cy.clamp(meia_h, 1.0 - meia_h);
    }

    /// Pinça: amplia `fator` vezes mantendo parado o ponto da tela sob os dedos.
    pub fn zoom(&mut self, fator: f32, x: f32, y: f32) {
        if !fator.is_finite() || fator <= 0.0 {
            return;
        }
        let antes = self.tela_para_fracao(x, y);
        self.escala = (self.escala * fator).clamp(1.0, ZOOM_MAX);
        let depois = self.tela_para_fracao(x, y);
        self.cx += antes.0 - depois.0;
        self.cy += antes.1 - depois.1;
        self.limitar();
    }

    /// Arrasta a janela `dx`, `dy` pixels da tela.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let img = self.imagem();
        self.cx -= dx / img.w;
        self.cy -= dy / img.h;
        self.limitar();
    }

    /// Volta a mostrar o monitor inteiro.
    pub fn restaurar(&mut self) {
        self.escala = 1.0;
        self.cx = 0.5;
        self.cy = 0.5;
    }

    /// No touchpad com zoom, a janela acompanha o cursor do host quando ele chega perto da
    /// borda (fração `nx`, `ny` do monitor).
    pub fn seguir(&mut self, nx: f32, ny: f32) {
        if self.escala <= 1.0 {
            return;
        }
        let img = self.imagem();
        let (vis_w, vis_h) = (self.area.w / img.w, self.area.h / img.h);
        let margem = 0.15;
        let (x0, x1) = (self.cx - vis_w / 2.0, self.cx + vis_w / 2.0);
        let (y0, y1) = (self.cy - vis_h / 2.0, self.cy + vis_h / 2.0);
        if nx < x0 + vis_w * margem {
            self.cx -= x0 + vis_w * margem - nx;
        } else if nx > x1 - vis_w * margem {
            self.cx += nx - (x1 - vis_w * margem);
        }
        if ny < y0 + vis_h * margem {
            self.cy -= y0 + vis_h * margem - ny;
        } else if ny > y1 - vis_h * margem {
            self.cy += ny - (y1 - vis_h * margem);
        }
        self.limitar();
    }

    fn tela_para_fracao(&self, x: f32, y: f32) -> (f32, f32) {
        let img = self.imagem();
        ((x - img.x) / img.w, (y - img.y) / img.h)
    }

    /// Ponto da tela → `AbsPos` do fio (0..=65535 no monitor). `None` fora da imagem (nas faixas
    /// pretas o toque não vira clique).
    pub fn tela_para_monitor(&self, x: f32, y: f32) -> Option<(u32, u32)> {
        let (fx, fy) = self.tela_para_fracao(x, y);
        if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
            return None;
        }
        Some(fracao_para_fio(fx, fy))
    }

    /// Deslocamento do touchpad em pixels da tela → deslocamento em fração do monitor. Com zoom
    /// o cursor anda menos (precisão), como numa lupa.
    pub fn delta_para_fracao(&self, dx: f32, dy: f32) -> (f32, f32) {
        // Um dedo atravessando a área inteira leva o cursor de uma ponta à outra do que se vê.
        let img = self.imagem();
        (dx / img.w, dy / img.h)
    }

    /// Fração do monitor → ponto na tela (para desenhar o cursor do host).
    pub fn fracao_para_tela(&self, fx: f32, fy: f32) -> (f32, f32) {
        let img = self.imagem();
        (img.x + fx * img.w, img.y + fy * img.h)
    }
}

/// Fração 0..=1 → coordenada do fio, arredondada e presa no limite.
pub fn fracao_para_fio(fx: f32, fy: f32) -> (u32, u32) {
    let c = |f: f32| (f.clamp(0.0, 1.0) * COORD_MAX as f32).round() as u32;
    (c(fx), c(fy))
}

/// Coordenada do fio → fração.
pub fn fio_para_fracao(x: u32, y: u32) -> (f32, f32) {
    (
        x.min(COORD_MAX) as f32 / COORD_MAX as f32,
        y.min(COORD_MAX) as f32 / COORD_MAX as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn celular() -> Vista {
        // Celular em paisagem 2400×1080; monitor 1920×1080: faixas de 240 px dos lados.
        Vista::new(
            Retangulo {
                x: 0.0,
                y: 0.0,
                w: 2400.0,
                h: 1080.0,
            },
            (1920, 1080),
        )
    }

    #[test]
    fn faixas_pretas_e_cantos() {
        let v = celular();
        assert_eq!(v.imagem().x, 240.0);
        assert_eq!(v.tela_para_monitor(240.0, 0.0), Some((0, 0)));
        assert_eq!(v.tela_para_monitor(2160.0, 1080.0), Some((65_535, 65_535)));
        assert_eq!(v.tela_para_monitor(1200.0, 540.0), Some((32_768, 32_768)));
        assert_eq!(v.tela_para_monitor(100.0, 540.0), None);
    }

    #[test]
    fn pinca_mantem_o_ponto_sob_os_dedos() {
        let mut v = celular();
        let alvo = v.tela_para_monitor(800.0, 300.0).unwrap();
        v.zoom(2.0, 800.0, 300.0);
        assert_eq!(v.escala(), 2.0);
        let (x, y) = v.tela_para_monitor(800.0, 300.0).unwrap();
        assert!(
            x.abs_diff(alvo.0) <= 2 && y.abs_diff(alvo.1) <= 2,
            "{x},{y} vs {alvo:?}"
        );
        // Zoom tem teto e piso.
        v.zoom(100.0, 0.0, 0.0);
        assert_eq!(v.escala(), ZOOM_MAX);
        v.zoom(0.0001, 0.0, 0.0);
        assert_eq!(v.escala(), 1.0);
        v.zoom(f32::NAN, 0.0, 0.0);
        assert_eq!(v.escala(), 1.0);
    }

    #[test]
    fn pan_nao_sai_do_monitor() {
        let mut v = celular();
        v.zoom(3.0, 1200.0, 540.0);
        v.pan(1e6, 1e6);
        // Canto superior esquerdo do monitor encostado no canto da tela.
        let img = v.imagem();
        assert!((img.y - 0.0).abs() < 0.5, "{img:?}");
        assert!(img.x <= 0.5);
        assert_eq!(v.tela_para_monitor(img.x.max(0.0), 0.0), Some((0, 0)));
        // Sem zoom não há pan.
        let mut v = celular();
        v.pan(500.0, 500.0);
        assert_eq!(v.imagem().x, 240.0);
    }

    #[test]
    fn girar_mantem_zoom_e_centro() {
        let mut v = celular();
        v.zoom(2.0, 600.0, 300.0);
        let antes = v.tela_para_monitor(1200.0, 540.0).unwrap();
        v.redimensionar(
            Retangulo {
                x: 0.0,
                y: 0.0,
                w: 1080.0,
                h: 2400.0,
            },
            (1920, 1080),
        );
        assert_eq!(v.escala(), 2.0);
        let depois = v.tela_para_monitor(540.0, 1200.0).unwrap();
        assert!(antes.0.abs_diff(depois.0) < 3_000, "{antes:?} {depois:?}");
    }

    #[test]
    fn janela_segue_o_cursor_perto_da_borda() {
        let mut v = celular();
        v.zoom(4.0, 1200.0, 540.0);
        let antes = v.imagem().x;
        v.seguir(0.99, 0.5);
        assert!(v.imagem().x < antes);
        let (sx, _) = v.fracao_para_tela(0.99, 0.5);
        assert!(sx <= 2400.0, "cursor visível: {sx}");
    }

    #[test]
    fn conversao_do_fio() {
        assert_eq!(fracao_para_fio(-1.0, 2.0), (0, 65_535));
        let (fx, fy) = fio_para_fracao(65_535, 0);
        assert_eq!((fx, fy), (1.0, 0.0));
    }
}
