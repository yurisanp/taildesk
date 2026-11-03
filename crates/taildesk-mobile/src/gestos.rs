//! Toques → ações (`mobile/arquitetura.md` §6, MF-40..42).
//!
//! Dois modos:
//! - **Touchpad** (padrão no celular): a tela é um touchpad; 1 dedo move o cursor, toque clica,
//!   tocar-segurar-arrastar arrasta, 2 dedos rolam (toque com 2 = botão direito), pinça dá zoom
//!   local, 3 dedos para o lado trocam de monitor (toque com 3 = botão do meio).
//! - **Toque direto** (padrão no tablet): o dedo é o mouse onde toca; toque clica ali, segurar dá
//!   o botão direito, arrastar arrasta. 2 e 3 dedos como no touchpad.
//!
//! A máquina não sabe de protocolo nem de tela: recebe [`Toque`] em pixels da tela do celular e
//! devolve [`Acao`]. Quem chama converte para `MouseMove`/`MouseButton`/`Scroll` pelo
//! [`crate::zoom::Vista`]. O tempo vem de fora (ms), então os testes são sequências gravadas.

/// Fase de um dedo, como o `MotionEvent` do Android.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fase {
    Desceu,
    Moveu,
    Subiu,
    /// O sistema tirou o gesto (notificação puxada, rotação): solta tudo sem clicar.
    Cancelado,
}

/// Um evento de dedo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Toque {
    pub id: i32,
    pub fase: Fase,
    pub x: f32,
    pub y: f32,
    pub t_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Botao {
    Esquerdo,
    Direito,
    Meio,
}

/// O que fazer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Acao {
    /// Touchpad: mover o cursor do host por este deslocamento (px da tela, já acelerado).
    Mover {
        dx: f32,
        dy: f32,
    },
    /// Toque direto: pôr o cursor neste ponto da tela.
    Apontar {
        x: f32,
        y: f32,
    },
    Clique(Botao),
    Pressionar(Botao),
    Soltar(Botao),
    /// Rolagem em pixels (positivo = para baixo/direita, como o `Scroll` do fio).
    Rolar {
        dx: f32,
        dy: f32,
    },
    /// Fim de um gesto de rolagem (`Scroll{0, 0}` no fio).
    FimRolagem,
    /// Zoom local em volta de (cx, cy): só a imagem muda, nada vai ao host.
    Zoom {
        fator: f32,
        cx: f32,
        cy: f32,
    },
    /// Trocar de monitor: +1 próximo, -1 anterior.
    TrocarMonitor(i32),
}

/// Modo de entrada.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Modo {
    #[default]
    Touchpad,
    ToqueDireto,
}

/// Limiares, em px e ms. `densidade` = px por dp (o Android dá em `DisplayMetrics.density`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limiares {
    pub arrasto_px: f32,
    pub toque_ms: u64,
    pub segurar_ms: u64,
    pub duplo_ms: u64,
    pub troca_monitor_px: f32,
}

impl Limiares {
    pub fn para_densidade(densidade: f32) -> Limiares {
        let d = densidade.max(0.5);
        Limiares {
            arrasto_px: 8.0 * d,
            toque_ms: 250,
            segurar_ms: 500,
            duplo_ms: 300,
            troca_monitor_px: 80.0 * d,
        }
    }
}

impl Default for Limiares {
    fn default() -> Self {
        Limiares::para_densidade(1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Dedo {
    id: i32,
    x0: f32,
    y0: f32,
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Estado {
    Parado,
    /// Um dedo, ainda sem passar do limiar.
    Um {
        t0: u64,
        depois_de_toque: bool,
    },
    /// Um dedo movendo o cursor (touchpad) ou arrastando com o botão apertado.
    Movendo {
        segurando: bool,
    },
    /// Dois dedos, ainda sem decidir.
    Dois {
        t0: u64,
        dist0: f32,
    },
    Rolando,
    Pinca {
        dist: f32,
    },
    Tres {
        t0: u64,
        trocou: bool,
    },
    /// Dedos saindo depois de um gesto de 2 ou 3: ignora até todos subirem.
    Esperando,
}

/// A máquina de gestos de uma sessão.
#[derive(Clone, Debug)]
pub struct Gestos {
    pub modo: Modo,
    pub limiares: Limiares,
    dedos: Vec<Dedo>,
    estado: Estado,
    ultimo_toque: Option<u64>,
    ultimo_t: u64,
}

impl Gestos {
    pub fn new(modo: Modo, limiares: Limiares) -> Self {
        Gestos {
            modo,
            limiares,
            dedos: Vec::with_capacity(5),
            estado: Estado::Parado,
            ultimo_toque: None,
            ultimo_t: 0,
        }
    }

    fn centro(&self) -> (f32, f32) {
        let n = self.dedos.len().max(1) as f32;
        let (sx, sy) = self
            .dedos
            .iter()
            .fold((0.0, 0.0), |(a, b), d| (a + d.x, b + d.y));
        (sx / n, sy / n)
    }

    fn distancia(&self) -> f32 {
        match self.dedos.as_slice() {
            [a, b, ..] => ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt().max(1.0),
            _ => 1.0,
        }
    }

    fn passou(&self, d: &Dedo) -> bool {
        (d.x - d.x0).abs().max((d.y - d.y0).abs()) > self.limiares.arrasto_px
    }

    /// Aceleração do touchpad: devagar = preciso, rápido = atravessa a tela.
    fn acelerar(&self, dx: f32, dy: f32, dt_ms: u64) -> (f32, f32) {
        let v = (dx * dx + dy * dy).sqrt() / dt_ms.max(1) as f32; // px/ms
        let ganho = 1.0 + (v / 1.5).min(2.0);
        (dx * ganho, dy * ganho)
    }

    /// Solta o que estiver apertado (cancelamento, sair da sessão).
    pub fn cancelar(&mut self, out: &mut Vec<Acao>) {
        match self.estado {
            Estado::Movendo { segurando: true } => out.push(Acao::Soltar(Botao::Esquerdo)),
            Estado::Rolando => out.push(Acao::FimRolagem),
            _ => {}
        }
        self.dedos.clear();
        self.estado = Estado::Parado;
    }

    /// Chame com o relógio de vez em quando (a cada quadro basta): o "segurar" sem mexer
    /// acontece sem evento de dedo.
    pub fn tick(&mut self, t_ms: u64, out: &mut Vec<Acao>) {
        if let Estado::Um { t0, .. } = self.estado
            && t_ms.saturating_sub(t0) >= self.limiares.segurar_ms
            && let Some(&d) = self.dedos.first()
        {
            match self.modo {
                // Segurar parado no touchpad começa um arrasto.
                Modo::Touchpad => {
                    out.push(Acao::Pressionar(Botao::Esquerdo));
                    self.estado = Estado::Movendo { segurando: true };
                }
                // No toque direto, segurar é o botão direito ali.
                Modo::ToqueDireto => {
                    out.push(Acao::Apontar { x: d.x, y: d.y });
                    out.push(Acao::Clique(Botao::Direito));
                    self.estado = Estado::Esperando;
                }
            }
        }
    }

    pub fn toque(&mut self, t: Toque, out: &mut Vec<Acao>) {
        let dt = t.t_ms.saturating_sub(self.ultimo_t);
        self.ultimo_t = t.t_ms;
        self.tick(t.t_ms, out);
        match t.fase {
            Fase::Cancelado => self.cancelar(out),
            Fase::Desceu => self.desceu(t, out),
            Fase::Moveu => self.moveu(t, dt, out),
            Fase::Subiu => self.subiu(t, out),
        }
    }

    fn desceu(&mut self, t: Toque, out: &mut Vec<Acao>) {
        if self.dedos.iter().any(|d| d.id == t.id) || self.dedos.len() >= 5 {
            return;
        }
        self.dedos.push(Dedo {
            id: t.id,
            x0: t.x,
            y0: t.y,
            x: t.x,
            y: t.y,
        });
        match (self.dedos.len(), self.estado) {
            (1, _) => {
                let depois_de_toque = self
                    .ultimo_toque
                    .is_some_and(|u| t.t_ms.saturating_sub(u) <= self.limiares.duplo_ms);
                self.estado = Estado::Um {
                    t0: t.t_ms,
                    depois_de_toque,
                };
            }
            (2, Estado::Um { .. }) | (2, Estado::Movendo { segurando: false }) => {
                for d in &mut self.dedos {
                    (d.x0, d.y0) = (d.x, d.y);
                }
                self.estado = Estado::Dois {
                    t0: t.t_ms,
                    dist0: self.distancia(),
                };
            }
            (3, Estado::Dois { .. }) | (3, Estado::Rolando) | (3, Estado::Pinca { .. }) => {
                if self.estado == Estado::Rolando {
                    out.push(Acao::FimRolagem);
                }
                for d in &mut self.dedos {
                    (d.x0, d.y0) = (d.x, d.y);
                }
                self.estado = Estado::Tres {
                    t0: t.t_ms,
                    trocou: false,
                };
            }
            _ => {}
        }
    }

    fn moveu(&mut self, t: Toque, dt: u64, out: &mut Vec<Acao>) {
        let antes = self.centro();
        let dist_antes = self.distancia();
        let Some(i) = self.dedos.iter().position(|d| d.id == t.id) else {
            return;
        };
        let (ox, oy) = (self.dedos[i].x, self.dedos[i].y);
        self.dedos[i].x = t.x;
        self.dedos[i].y = t.y;
        let dedo = self.dedos[i];
        match self.estado {
            Estado::Um {
                depois_de_toque, ..
            } => {
                if !self.passou(&dedo) {
                    return;
                }
                match self.modo {
                    Modo::Touchpad => {
                        // Toque e logo depois arrastar: arrasta com o botão apertado.
                        if depois_de_toque {
                            out.push(Acao::Pressionar(Botao::Esquerdo));
                        }
                        let (dx, dy) = self.acelerar(t.x - dedo.x0, t.y - dedo.y0, dt);
                        out.push(Acao::Mover { dx, dy });
                        self.estado = Estado::Movendo {
                            segurando: depois_de_toque,
                        };
                    }
                    Modo::ToqueDireto => {
                        out.push(Acao::Apontar {
                            x: dedo.x0,
                            y: dedo.y0,
                        });
                        out.push(Acao::Pressionar(Botao::Esquerdo));
                        out.push(Acao::Apontar { x: t.x, y: t.y });
                        self.estado = Estado::Movendo { segurando: true };
                    }
                }
            }
            Estado::Movendo { .. } => match self.modo {
                Modo::Touchpad => {
                    let (dx, dy) = self.acelerar(t.x - ox, t.y - oy, dt);
                    out.push(Acao::Mover { dx, dy });
                }
                Modo::ToqueDireto => out.push(Acao::Apontar { x: t.x, y: t.y }),
            },
            Estado::Dois { dist0, .. } => {
                if self.dedos.len() < 2 {
                    return;
                }
                let razao = self.distancia() / dist0;
                if (razao - 1.0).abs() > 0.12 {
                    self.estado = Estado::Pinca {
                        dist: self.distancia(),
                    };
                    let (cx, cy) = self.centro();
                    out.push(Acao::Zoom {
                        fator: razao,
                        cx,
                        cy,
                    });
                } else if self.dedos.iter().take(2).any(|d| self.passou(d)) {
                    self.estado = Estado::Rolando;
                    let (cx, cy) = self.centro();
                    let (x0, y0) = self
                        .dedos
                        .iter()
                        .take(2)
                        .fold((0.0, 0.0), |(a, b), d| (a + d.x0 / 2.0, b + d.y0 / 2.0));
                    // O conteúdo segue os dedos: dedos para cima = rolar para baixo.
                    out.push(Acao::Rolar {
                        dx: x0 - cx,
                        dy: y0 - cy,
                    });
                }
            }
            Estado::Rolando => {
                let (cx, cy) = self.centro();
                let (dx, dy) = (antes.0 - cx, antes.1 - cy);
                if dx != 0.0 || dy != 0.0 {
                    out.push(Acao::Rolar { dx, dy });
                }
            }
            Estado::Pinca { dist } => {
                let nova = self.distancia();
                let (cx, cy) = self.centro();
                if (nova - dist).abs() > 0.5 {
                    out.push(Acao::Zoom {
                        fator: nova / dist,
                        cx,
                        cy,
                    });
                    self.estado = Estado::Pinca { dist: nova };
                }
                let _ = dist_antes;
            }
            Estado::Tres { t0, trocou: false } => {
                let (cx, _) = self.centro();
                let x0 = self.dedos.iter().map(|d| d.x0).sum::<f32>() / self.dedos.len() as f32;
                let dx = cx - x0;
                if dx.abs() >= self.limiares.troca_monitor_px {
                    // Dedos para a esquerda = próximo monitor (como passar página).
                    out.push(Acao::TrocarMonitor(if dx < 0.0 { 1 } else { -1 }));
                    self.estado = Estado::Tres { t0, trocou: true };
                }
            }
            Estado::Tres { trocou: true, .. } | Estado::Parado | Estado::Esperando => {}
        }
    }

    fn subiu(&mut self, t: Toque, out: &mut Vec<Acao>) {
        let Some(i) = self.dedos.iter().position(|d| d.id == t.id) else {
            return;
        };
        let dedo = self.dedos.remove(i);
        let rapido = |t0: u64| t.t_ms.saturating_sub(t0) <= self.limiares.toque_ms;
        match self.estado {
            Estado::Um { t0, .. } => {
                if rapido(t0) || self.modo == Modo::ToqueDireto {
                    if self.modo == Modo::ToqueDireto {
                        out.push(Acao::Apontar {
                            x: dedo.x0,
                            y: dedo.y0,
                        });
                    }
                    out.push(Acao::Clique(Botao::Esquerdo));
                    self.ultimo_toque = Some(t.t_ms);
                }
                self.estado = Estado::Parado;
            }
            Estado::Movendo { segurando } => {
                if segurando {
                    out.push(Acao::Soltar(Botao::Esquerdo));
                }
                self.ultimo_toque = None;
                self.estado = Estado::Parado;
            }
            Estado::Dois { t0, .. } => {
                if rapido(t0) {
                    out.push(Acao::Clique(Botao::Direito));
                }
                self.estado = Estado::Esperando;
            }
            Estado::Rolando => {
                out.push(Acao::FimRolagem);
                self.estado = Estado::Esperando;
            }
            Estado::Pinca { .. } => self.estado = Estado::Esperando,
            Estado::Tres { t0, trocou } => {
                if !trocou && rapido(t0) {
                    out.push(Acao::Clique(Botao::Meio));
                }
                self.estado = Estado::Esperando;
            }
            Estado::Parado | Estado::Esperando => {}
        }
        if self.dedos.is_empty() {
            self.estado = Estado::Parado;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grava uma sequência de toques e devolve as ações.
    struct Gravacao {
        g: Gestos,
        t: u64,
        out: Vec<Acao>,
    }

    impl Gravacao {
        fn new(modo: Modo) -> Self {
            Gravacao {
                g: Gestos::new(modo, Limiares::default()),
                t: 1_000,
                out: Vec::new(),
            }
        }
        fn ev(&mut self, id: i32, fase: Fase, x: f32, y: f32, dt: u64) -> &mut Self {
            self.t += dt;
            let t = Toque {
                id,
                fase,
                x,
                y,
                t_ms: self.t,
            };
            self.g.toque(t, &mut self.out);
            self
        }
        fn tick(&mut self, dt: u64) -> &mut Self {
            self.t += dt;
            self.g.tick(self.t, &mut self.out);
            self
        }
        fn tirar(&mut self) -> Vec<Acao> {
            std::mem::take(&mut self.out)
        }
    }

    use Fase::*;

    #[test]
    fn toque_clica_e_mexer_um_pouco_ainda_e_toque() {
        let mut g = Gravacao::new(Modo::Touchpad);
        g.ev(0, Desceu, 100.0, 100.0, 0)
            .ev(0, Moveu, 103.0, 102.0, 30)
            .ev(0, Subiu, 103.0, 102.0, 40);
        assert_eq!(g.tirar(), vec![Acao::Clique(Botao::Esquerdo)]);
        // Lento demais não clica (a pessoa desistiu).
        g.ev(0, Desceu, 100.0, 100.0, 1_000)
            .ev(0, Subiu, 100.0, 100.0, 400);
        assert!(g.tirar().is_empty());
    }

    #[test]
    fn um_dedo_move_o_cursor_com_aceleracao() {
        let mut g = Gravacao::new(Modo::Touchpad);
        g.ev(0, Desceu, 100.0, 100.0, 0)
            .ev(0, Moveu, 120.0, 100.0, 16)
            .ev(0, Moveu, 121.0, 100.0, 16)
            .ev(0, Subiu, 121.0, 100.0, 16);
        let a = g.tirar();
        assert_eq!(a.len(), 2, "{a:?}");
        let Acao::Mover { dx: rapido, .. } = a[0] else {
            panic!("{a:?}")
        };
        let Acao::Mover { dx: lento, .. } = a[1] else {
            panic!("{a:?}")
        };
        assert!(rapido > 20.0, "rápido acelera: {rapido}");
        assert!((lento - 1.0).abs() < 0.1, "devagar é preciso: {lento}");
    }

    #[test]
    fn tocar_e_arrastar_seleciona() {
        let mut g = Gravacao::new(Modo::Touchpad);
        g.ev(0, Desceu, 100.0, 100.0, 0)
            .ev(0, Subiu, 100.0, 100.0, 60)
            .ev(0, Desceu, 100.0, 100.0, 120)
            .ev(0, Moveu, 150.0, 100.0, 50)
            .ev(0, Subiu, 150.0, 100.0, 50);
        let a = g.tirar();
        assert_eq!(a[0], Acao::Clique(Botao::Esquerdo));
        assert_eq!(a[1], Acao::Pressionar(Botao::Esquerdo));
        assert!(matches!(a[2], Acao::Mover { .. }));
        assert_eq!(a.last(), Some(&Acao::Soltar(Botao::Esquerdo)));
    }

    #[test]
    fn segurar_parado_comeca_arrasto() {
        let mut g = Gravacao::new(Modo::Touchpad);
        g.ev(0, Desceu, 100.0, 100.0, 0).tick(600);
        assert_eq!(g.tirar(), vec![Acao::Pressionar(Botao::Esquerdo)]);
        g.ev(0, Moveu, 140.0, 100.0, 16)
            .ev(0, Subiu, 140.0, 100.0, 16);
        let a = g.tirar();
        assert!(matches!(a[0], Acao::Mover { .. }));
        assert_eq!(a[1], Acao::Soltar(Botao::Esquerdo));
    }

    #[test]
    fn dois_dedos_rolam_e_tocar_com_dois_e_botao_direito() {
        let mut g = Gravacao::new(Modo::Touchpad);
        g.ev(0, Desceu, 100.0, 300.0, 0)
            .ev(1, Desceu, 200.0, 300.0, 10)
            .ev(0, Subiu, 100.0, 300.0, 80)
            .ev(1, Subiu, 200.0, 300.0, 10);
        assert_eq!(g.tirar(), vec![Acao::Clique(Botao::Direito)]);

        // Dedos sobem 60 px: conteúdo rola para baixo (dy positivo) e o gesto termina.
        g.ev(0, Desceu, 100.0, 300.0, 1_000)
            .ev(1, Desceu, 200.0, 300.0, 5)
            .ev(0, Moveu, 100.0, 270.0, 16)
            .ev(1, Moveu, 200.0, 270.0, 1)
            .ev(0, Moveu, 100.0, 240.0, 16)
            .ev(1, Moveu, 200.0, 240.0, 1)
            .ev(0, Subiu, 100.0, 240.0, 16)
            .ev(1, Subiu, 200.0, 240.0, 1);
        let a = g.tirar();
        let total: f32 = a
            .iter()
            .filter_map(|x| match x {
                Acao::Rolar { dy, .. } => Some(*dy),
                _ => None,
            })
            .sum();
        assert!((total - 60.0).abs() < 0.01, "{a:?}");
        assert_eq!(a.last(), Some(&Acao::FimRolagem));
        assert!(!a.iter().any(|x| matches!(x, Acao::Clique(_))));
    }

    #[test]
    fn pinca_e_zoom_local() {
        let mut g = Gravacao::new(Modo::Touchpad);
        g.ev(0, Desceu, 200.0, 300.0, 0)
            .ev(1, Desceu, 300.0, 300.0, 5)
            .ev(0, Moveu, 150.0, 300.0, 16)
            .ev(1, Moveu, 350.0, 300.0, 16)
            .ev(0, Subiu, 150.0, 300.0, 16)
            .ev(1, Subiu, 350.0, 300.0, 1);
        let a = g.tirar();
        let fator: f32 = a
            .iter()
            .filter_map(|x| match x {
                Acao::Zoom { fator, .. } => Some(*fator),
                _ => None,
            })
            .product();
        assert!((fator - 2.0).abs() < 0.01, "{a:?}");
        assert!(a.iter().all(|x| matches!(x, Acao::Zoom { .. })));
    }

    #[test]
    fn tres_dedos_trocam_de_monitor_ou_clicam_o_meio() {
        let mut g = Gravacao::new(Modo::Touchpad);
        g.ev(0, Desceu, 300.0, 300.0, 0)
            .ev(1, Desceu, 400.0, 300.0, 5)
            .ev(2, Desceu, 500.0, 300.0, 5);
        for i in 1..=10 {
            let dx = -12.0 * i as f32;
            g.ev(0, Moveu, 300.0 + dx, 300.0, 16)
                .ev(1, Moveu, 400.0 + dx, 300.0, 0)
                .ev(2, Moveu, 500.0 + dx, 300.0, 0);
        }
        g.ev(0, Subiu, 180.0, 300.0, 16)
            .ev(1, Subiu, 280.0, 300.0, 0)
            .ev(2, Subiu, 380.0, 300.0, 0);
        assert_eq!(g.tirar(), vec![Acao::TrocarMonitor(1)]);

        g.ev(0, Desceu, 300.0, 300.0, 1_000)
            .ev(1, Desceu, 400.0, 300.0, 5)
            .ev(2, Desceu, 500.0, 300.0, 5)
            .ev(2, Subiu, 500.0, 300.0, 60)
            .ev(1, Subiu, 400.0, 300.0, 0)
            .ev(0, Subiu, 300.0, 300.0, 0);
        assert_eq!(g.tirar(), vec![Acao::Clique(Botao::Meio)]);
    }

    #[test]
    fn toque_direto_aponta_clica_e_arrasta() {
        let mut g = Gravacao::new(Modo::ToqueDireto);
        g.ev(0, Desceu, 50.0, 60.0, 0).ev(0, Subiu, 50.0, 60.0, 80);
        assert_eq!(
            g.tirar(),
            vec![
                Acao::Apontar { x: 50.0, y: 60.0 },
                Acao::Clique(Botao::Esquerdo)
            ]
        );
        g.ev(0, Desceu, 50.0, 60.0, 1_000).tick(600);
        assert_eq!(
            g.tirar(),
            vec![
                Acao::Apontar { x: 50.0, y: 60.0 },
                Acao::Clique(Botao::Direito)
            ]
        );
        g.ev(0, Subiu, 50.0, 60.0, 10);
        assert!(
            g.tirar().is_empty(),
            "soltar depois do botão direito não clica"
        );

        g.ev(0, Desceu, 10.0, 10.0, 1_000)
            .ev(0, Moveu, 60.0, 10.0, 16)
            .ev(0, Subiu, 60.0, 10.0, 16);
        assert_eq!(
            g.tirar(),
            vec![
                Acao::Apontar { x: 10.0, y: 10.0 },
                Acao::Pressionar(Botao::Esquerdo),
                Acao::Apontar { x: 60.0, y: 10.0 },
                Acao::Soltar(Botao::Esquerdo),
            ]
        );
    }

    #[test]
    fn cancelado_solta_sem_clicar() {
        let mut g = Gravacao::new(Modo::Touchpad);
        g.ev(0, Desceu, 100.0, 100.0, 0).tick(600);
        g.tirar();
        g.ev(0, Cancelado, 100.0, 100.0, 10);
        assert_eq!(g.tirar(), vec![Acao::Soltar(Botao::Esquerdo)]);
        // Depois do cancelamento, a máquina recomeça limpa.
        g.ev(0, Desceu, 100.0, 100.0, 1_000)
            .ev(0, Subiu, 100.0, 100.0, 50);
        assert_eq!(g.tirar(), vec![Acao::Clique(Botao::Esquerdo)]);
    }

    #[test]
    fn eventos_estranhos_nao_quebram() {
        let mut g = Gravacao::new(Modo::Touchpad);
        g.ev(7, Subiu, 1.0, 1.0, 0)
            .ev(7, Moveu, 1.0, 1.0, 0)
            .ev(0, Desceu, 1.0, 1.0, 0)
            .ev(0, Desceu, 1.0, 1.0, 0);
        for id in 1..9 {
            g.ev(id, Desceu, id as f32, 1.0, 1);
        }
        for id in 0..9 {
            g.ev(id, Subiu, id as f32, 1.0, 1);
        }
        g.tirar();
        g.ev(0, Desceu, 100.0, 100.0, 1_000)
            .ev(0, Subiu, 100.0, 100.0, 50);
        assert_eq!(g.tirar(), vec![Acao::Clique(Botao::Esquerdo)]);
    }
}
