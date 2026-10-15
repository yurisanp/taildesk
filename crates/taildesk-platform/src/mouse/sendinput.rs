//! Mouse no Windows com `SendInput`.
//!
//! A posição absoluta vai normalizada sobre a área de trabalho virtual
//! (`MOUSEEVENTF_VIRTUALDESK`), então quem chama passa o retângulo do monitor do `display_id`
//! em pixels físicos, no mesmo espaço da captura (o processo precisa ser DPI-aware por monitor).

use std::io;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    WHEEL_DELTA, XBUTTON1, XBUTTON2,
};

use super::{Botao, EventoMouse};

/// Pixels de rolagem precisa que valem um dente de roda (`WHEEL_DELTA`): 3 linhas de ~16 px,
/// o padrão do Windows.
const PX_POR_DENTE: i32 = 48;

/// Retângulo em pixels físicos da área de trabalho virtual.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retangulo {
    pub x: i32,
    pub y: i32,
    pub largura: i32,
    pub altura: i32,
}

/// A área de trabalho virtual (todos os monitores) agora.
pub fn area_virtual() -> Retangulo {
    // SAFETY: sem pré-condições.
    unsafe {
        Retangulo {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            largura: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            altura: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Coordenada do protocolo (0..=65535 no monitor) → pixel do monitor → coordenada absoluta do
/// `SendInput` (0..=65535 na área virtual). O Windows calcula `pixel = n * tamanho / 65536`
/// truncando, então o arredondamento para cima cai exatamente no pixel.
fn normalizar(v: u16, inicio: i32, tamanho: i32, v_inicio: i32, v_tamanho: i32) -> i32 {
    let tamanho = i64::from(tamanho.max(1));
    let v_tamanho = i64::from(v_tamanho.max(1));
    let px = i64::from(inicio) + (i64::from(v) * (tamanho - 1) + 32767) / 65535;
    let rel = (px - i64::from(v_inicio)).clamp(0, v_tamanho - 1);
    ((rel * 65536 + v_tamanho - 1) / v_tamanho).min(65535) as i32
}

fn absoluto(x: u16, y: u16, monitor: Retangulo, area: Retangulo) -> (i32, i32) {
    (
        normalizar(x, monitor.x, monitor.largura, area.x, area.largura),
        normalizar(y, monitor.y, monitor.altura, area.y, area.altura),
    )
}

fn entrada(dx: i32, dy: i32, dados: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: dados,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Injetor do Windows. Guarda o resto da rolagem precisa entre eventos.
#[derive(Default)]
pub struct InjetorMouseWindows {
    resto: (i32, i32),
}

impl std::fmt::Debug for InjetorMouseWindows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InjetorMouseWindows")
            .finish_non_exhaustive()
    }
}

impl InjetorMouseWindows {
    /// Monta os `INPUT` de um evento (no máximo dois). `monitor` é o retângulo do `display_id`
    /// de um [`EventoMouse::Absoluto`]; os outros eventos o ignoram.
    fn entradas(
        &mut self,
        ev: &EventoMouse,
        monitor: Retangulo,
        area: Retangulo,
    ) -> ([INPUT; 2], usize) {
        let vazio = entrada(0, 0, 0, 0);
        let um = |i| ([i, vazio], 1);
        match *ev {
            EventoMouse::Absoluto { x, y, .. } => {
                let (nx, ny) = absoluto(x, y, monitor, area);
                um(entrada(
                    nx,
                    ny,
                    0,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                ))
            }
            EventoMouse::Relativo { dx, dy } => um(entrada(dx, dy, 0, MOUSEEVENTF_MOVE)),
            EventoMouse::Botao { botao, down } => {
                let (flags, dados) = match (botao, down) {
                    (Botao::Esquerdo, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                    (Botao::Esquerdo, false) => (MOUSEEVENTF_LEFTUP, 0),
                    (Botao::Direito, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                    (Botao::Direito, false) => (MOUSEEVENTF_RIGHTUP, 0),
                    (Botao::Meio, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                    (Botao::Meio, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                    (Botao::Voltar, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
                    (Botao::Voltar, false) => (MOUSEEVENTF_XUP, XBUTTON1),
                    (Botao::Avancar, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
                    (Botao::Avancar, false) => (MOUSEEVENTF_XUP, XBUTTON2),
                };
                um(entrada(0, 0, u32::from(dados), flags))
            }
            EventoMouse::Rolagem { dx, dy, precisa } => {
                let delta = WHEEL_DELTA as i32;
                let (wx, wy) = if precisa {
                    let (rx, ry) = (
                        self.resto.0.saturating_add(dx.saturating_mul(delta)),
                        self.resto.1.saturating_add(dy.saturating_mul(delta)),
                    );
                    self.resto = (rx % PX_POR_DENTE, ry % PX_POR_DENTE);
                    if dx == 0 && dy == 0 {
                        self.resto = (0, 0);
                    }
                    (rx / PX_POR_DENTE, ry / PX_POR_DENTE)
                } else {
                    (dx.saturating_mul(delta), dy.saturating_mul(delta))
                };
                // No Windows a roda vertical positiva sobe; no protocolo positivo desce.
                let mut out = [vazio, vazio];
                let mut n = 0;
                if wy != 0 {
                    out[n] = entrada(0, 0, wy.saturating_neg() as u32, MOUSEEVENTF_WHEEL);
                    n += 1;
                }
                if wx != 0 {
                    out[n] = entrada(0, 0, wx as u32, MOUSEEVENTF_HWHEEL);
                    n += 1;
                }
                (out, n)
            }
        }
    }

    pub fn injetar(&mut self, ev: &EventoMouse, monitor: Retangulo) -> io::Result<()> {
        let (buf, n) = self.entradas(ev, monitor, area_virtual());
        if n == 0 {
            return Ok(());
        }
        // SAFETY: `buf` tem ao menos `n` estruturas INPUT válidas e o tamanho é o do tipo.
        let enviados = unsafe { SendInput(n as u32, buf.as_ptr(), size_of::<INPUT>() as i32) };
        if enviados as usize == n {
            Ok(())
        } else {
            // UIPI bloqueia injeção numa janela elevada (UAC) a partir de um processo comum.
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R1: Retangulo = Retangulo {
        x: 0,
        y: 0,
        largura: 1920,
        altura: 1080,
    };

    /// Pixel que o Windows calcula para a coordenada absoluta.
    fn pixel(n: i32, inicio: i32, tamanho: i32) -> i32 {
        inicio + (i64::from(n) * i64::from(tamanho) / 65536) as i32
    }

    #[test]
    fn absoluto_cai_no_pixel_certo() {
        // Monitor à esquerda do principal: a área virtual começa em -1280.
        let esq = Retangulo {
            x: -1280,
            y: 0,
            largura: 1280,
            altura: 1024,
        };
        let area = Retangulo {
            x: -1280,
            y: 0,
            largura: 3200,
            altura: 1080,
        };
        for (m, x, px) in [
            (R1, 0, 0),
            (R1, 65535, 1919),
            (R1, 32768, 960),
            (esq, 0, -1280),
            (esq, 65535, -1),
        ] {
            let (nx, _) = absoluto(x, 0, m, area);
            assert!((0..=65535).contains(&nx));
            assert_eq!(pixel(nx, area.x, area.largura), px, "x={x}");
        }
        let (_, ny) = absoluto(0, 65535, R1, area);
        assert_eq!(pixel(ny, area.y, area.altura), 1079);
    }

    fn mi(i: &INPUT) -> (i32, i32, u32, u32) {
        // SAFETY: todas as entradas montadas aqui são de mouse.
        let m = unsafe { i.Anonymous.mi };
        (m.dx, m.dy, m.mouseData, m.dwFlags)
    }

    fn montar(inj: &mut InjetorMouseWindows, ev: EventoMouse) -> Vec<(i32, i32, u32, u32)> {
        let (buf, n) = inj.entradas(&ev, R1, R1);
        buf[..n].iter().map(mi).collect()
    }

    #[test]
    fn botoes_e_rolagem() {
        let mut inj = InjetorMouseWindows::default();
        let botao = |botao, down| EventoMouse::Botao { botao, down };
        assert_eq!(
            montar(&mut inj, botao(Botao::Avancar, true)),
            [(0, 0, 2, MOUSEEVENTF_XDOWN)]
        );
        assert_eq!(
            montar(&mut inj, botao(Botao::Esquerdo, false)),
            [(0, 0, 0, MOUSEEVENTF_LEFTUP)]
        );
        let dentes = EventoMouse::Rolagem {
            dx: 1,
            dy: 2,
            precisa: false,
        };
        assert_eq!(
            montar(&mut inj, dentes),
            [
                (0, 0, (-240i32) as u32, MOUSEEVENTF_WHEEL),
                (0, 0, 120, MOUSEEVENTF_HWHEEL)
            ]
        );
        // 1 px vale 2,5 do delta: o meio que sobra passa para o próximo evento.
        let px = |dy| EventoMouse::Rolagem {
            dx: 0,
            dy,
            precisa: true,
        };
        assert_eq!(
            montar(&mut inj, px(1)),
            [(0, 0, (-2i32) as u32, MOUSEEVENTF_WHEEL)]
        );
        assert_eq!(
            montar(&mut inj, px(1)),
            [(0, 0, (-3i32) as u32, MOUSEEVENTF_WHEEL)]
        );
        assert_eq!(
            montar(&mut inj, px(1)),
            [(0, 0, (-2i32) as u32, MOUSEEVENTF_WHEEL)]
        );
        assert_eq!(montar(&mut inj, px(0)), []);
        assert_eq!(inj.resto, (0, 0));
    }
}
