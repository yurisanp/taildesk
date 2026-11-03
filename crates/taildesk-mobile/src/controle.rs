//! A sessão vista do celular, sem rede nem tela: o que o toque, a faixa de teclas e o teclado
//! virtual viram no fio (`MouseMove`, `MouseButton`, `Scroll`, `KeyChar`, `KeyShortcut`, `Key`,
//! `SecureAttention`, `StartVideo`/`StopVideo`), mais o zoom local e a troca de monitor.
//!
//! Regras do `AGENTS.md` que valem aqui: identidade nunca vem das mensagens; ao sair ou perder
//! o foco sempre `ReleaseAll` (MF-50); só um monitor recebido por vez no celular (MF-47).

use taildesk_keyboard::virtual_kb::{TeclaFaixa, Trava, VirtualKeyboard};
use taildesk_keyboard::{KeyMsg, Mods};
use taildesk_proto::control::{
    AbsPos, Button, ClientKind, CodecCap, CursorPos, DisplayInfo, Feature, Hello, KeyboardInfo,
    KeyboardSource, MouseButton, MouseMove, Msg, Platform, Priority, QualityPrefs, ReleaseAll,
    Scroll, SecureAttention, StartVideo, StopVideo, Viewport, mouse_move,
};

use crate::gestos::{Acao, Botao, Gestos, Limiares, Modo, Toque};
use crate::zoom::{Retangulo, Vista, fio_para_fracao, fracao_para_fio};

/// O que o celular diz de si no `Hello`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aparelho {
    pub modelo: String,
    pub arch: String,
    pub tablet: bool,
    /// Tela em pixels físicos e densidade.
    pub largura_px: u32,
    pub altura_px: u32,
    pub dpi: u32,
    /// Decode H.264 por hardware (MediaCodec) disponível.
    pub decode_hw: bool,
    /// Idioma do teclado do sistema ("pt-BR"), só para exibir no host.
    pub idioma_teclado: String,
    pub versao_app: String,
}

/// `Hello` de um celular: tipo, viewport, teclado virtual e o que decodifica.
pub fn hello(a: &Aparelho) -> Hello {
    Hello {
        app_version: a.versao_app.clone(),
        platform: Some(Platform {
            os: "android".into(),
            arch: a.arch.clone(),
            device_model: a.modelo.chars().take(64).collect(),
        }),
        decode_caps: vec![CodecCap {
            codec: taildesk_proto::control::Codec::H264.into(),
            profile: 66,
            max_width: a.largura_px.max(a.altura_px).min(4096),
            max_height: a.largura_px.min(a.altura_px).min(4096),
            chroma444: false,
            hw: a.decode_hw,
            gpu_output: a.decode_hw,
        }],
        // Arquivos e som vêm na M3; aqui só o que o celular já trata.
        features: vec![Feature::MultiDisplay.into()],
        keyboard: Some(KeyboardInfo {
            os_layout_id: format!("android:{}", a.idioma_teclado),
            layout_tag: String::new(),
            extra_hid: Vec::new(),
            source: KeyboardSource::Virtual.into(),
        }),
        client_kind: if a.tablet {
            ClientKind::Tablet
        } else {
            ClientKind::Phone
        }
        .into(),
        viewport: Some(Viewport {
            w_px: a
                .largura_px
                .min(taildesk_proto::control::limits::VIEWPORT_PX),
            h_px: a
                .altura_px
                .min(taildesk_proto::control::limits::VIEWPORT_PX),
            dpi: a.dpi,
        }),
        ..Hello::default()
    }
}

fn botao(b: Botao) -> Button {
    match b {
        Botao::Esquerdo => Button::Left,
        Botao::Direito => Button::Right,
        Botao::Meio => Button::Middle,
    }
}

/// Monitores do host, um por vez (MF-47).
#[derive(Clone, Debug, Default)]
pub struct Monitores {
    lista: Vec<DisplayInfo>,
    atual: Option<u32>,
}

impl Monitores {
    /// Monitores liberados pelo host (`shared`).
    pub fn lista(&self) -> &[DisplayInfo] {
        &self.lista
    }

    pub fn atual(&self) -> Option<&DisplayInfo> {
        self.atual
            .and_then(|id| self.lista.iter().find(|d| d.id == id))
    }

    /// Posição do atual e total ("1/2" na barra).
    pub fn posicao(&self) -> (usize, usize) {
        let i = self
            .atual
            .and_then(|id| self.lista.iter().position(|d| d.id == id))
            .map_or(0, |i| i + 1);
        (i, self.lista.len())
    }

    /// Lista nova (`HelloAck` ou `DisplaysChanged`). Começa (ou continua) no principal.
    /// Devolve o que pedir se o monitor atual mudou ou sumiu.
    pub fn definir(
        &mut self,
        todos: &[DisplayInfo],
        prefs: Option<QualityPrefs>,
        out: &mut Vec<Msg>,
    ) {
        self.lista = todos.iter().filter(|d| d.shared).cloned().collect();
        let ainda = self
            .atual
            .is_some_and(|id| self.lista.iter().any(|d| d.id == id));
        if ainda {
            return;
        }
        let novo = self
            .lista
            .iter()
            .find(|d| d.primary)
            .or(self.lista.first())
            .map(|d| d.id);
        // O antigo sumiu: o host já parou o stream (`VideoStopped{DISPLAY_REMOVED}`).
        self.atual = novo;
        if let Some(id) = novo {
            out.push(Msg::StartVideo(StartVideo {
                display_id: id,
                wanted: prefs,
                priority: Priority::Focused.into(),
            }));
        }
    }

    /// ◀ ▶ ou três dedos: para um e começa o outro.
    pub fn trocar(&mut self, passo: i32, prefs: Option<QualityPrefs>, out: &mut Vec<Msg>) {
        let n = self.lista.len();
        if n < 2 {
            return;
        }
        let (i, _) = self.posicao();
        let atual = i.saturating_sub(1) as i64;
        let proximo = (atual + i64::from(passo)).rem_euclid(n as i64) as usize;
        self.ir_para(self.lista[proximo].id, prefs, out);
    }

    /// Escolheu na lista da barra.
    pub fn ir_para(&mut self, id: u32, prefs: Option<QualityPrefs>, out: &mut Vec<Msg>) {
        if self.atual == Some(id) || !self.lista.iter().any(|d| d.id == id) {
            return;
        }
        if let Some(antigo) = self.atual {
            out.push(Msg::StopVideo(StopVideo { display_id: antigo }));
        }
        self.atual = Some(id);
        out.push(Msg::StartVideo(StartVideo {
            display_id: id,
            wanted: prefs,
            priority: Priority::Focused.into(),
        }));
    }
}

/// Estado do controle numa sessão.
#[derive(Debug)]
pub struct Controle {
    pub gestos: Gestos,
    pub vista: Vista,
    pub teclado: VirtualKeyboard,
    pub monitores: Monitores,
    /// Área de vídeo na tela do celular.
    area: Retangulo,
    /// Cursor do host em fração do monitor atual (do `CursorPos`, e movido localmente no
    /// touchpad para não esperar a volta).
    cursor: (f32, f32),
    /// O host atende Ctrl+Alt+Del de verdade (`HelloAck.secure_attention`).
    pub atencao_segura: bool,
    /// Qualidade pedida em cada `StartVideo` (rede tarifada, `crate::rede`).
    pub prefs: Option<QualityPrefs>,
    acoes: Vec<Acao>,
    teclas: Vec<KeyMsg>,
}

impl Controle {
    pub fn new(modo: Modo, densidade: f32, area: Retangulo) -> Controle {
        Controle {
            gestos: Gestos::new(modo, Limiares::para_densidade(densidade)),
            vista: Vista::new(area, (1920, 1080)),
            teclado: VirtualKeyboard::default(),
            monitores: Monitores::default(),
            area,
            cursor: (0.5, 0.5),
            atencao_segura: false,
            prefs: None,
            acoes: Vec::with_capacity(8),
            teclas: Vec::with_capacity(8),
        }
    }

    pub fn cursor(&self) -> (f32, f32) {
        self.cursor
    }

    fn display_id(&self) -> u32 {
        self.monitores.atual().map_or(0, |d| d.id)
    }

    /// Lista de monitores nova: ajusta a vista ao tamanho do atual.
    pub fn monitores_mudaram(&mut self, todos: &[DisplayInfo], out: &mut Vec<Msg>) {
        self.monitores.definir(todos, self.prefs, out);
        self.ajustar_vista();
    }

    /// Ajusta a vista ao tamanho do monitor atual (depois de trocar de monitor).
    pub fn ajustar_vista(&mut self) {
        if let Some(d) = self.monitores.atual() {
            self.vista.redimensionar(self.area, (d.width, d.height));
        }
    }

    /// Posição do cursor do host (H→C).
    pub fn cursor_do_host(&mut self, c: &CursorPos) {
        if c.display_id == self.display_id() {
            self.cursor = fio_para_fracao(c.x, c.y);
            if self.gestos.modo == Modo::Touchpad {
                self.vista.seguir(self.cursor.0, self.cursor.1);
            }
        }
    }

    /// A área de vídeo mudou (rotação, barra do teclado abriu). Zoom e centro ficam (MF-49).
    pub fn area_mudou(&mut self, area: Retangulo) {
        self.area = area;
        let tamanho = self
            .monitores
            .atual()
            .map_or((1920, 1080), |d| (d.width, d.height));
        self.vista.redimensionar(area, tamanho);
    }

    fn mover_abs(&mut self, fx: f32, fy: f32, out: &mut Vec<Msg>) {
        self.cursor = (fx.clamp(0.0, 1.0), fy.clamp(0.0, 1.0));
        let (x, y) = fracao_para_fio(self.cursor.0, self.cursor.1);
        out.push(Msg::MouseMove(MouseMove {
            pos: Some(mouse_move::Pos::Abs(AbsPos {
                display_id: self.display_id(),
                x,
                y,
            })),
        }));
    }

    fn acao(&mut self, a: Acao, out: &mut Vec<Msg>) {
        match a {
            Acao::Mover { dx, dy } => {
                let (fx, fy) = self.vista.delta_para_fracao(dx, dy);
                let (cx, cy) = self.cursor;
                self.mover_abs(cx + fx, cy + fy, out);
                self.vista.seguir(self.cursor.0, self.cursor.1);
            }
            Acao::Apontar { x, y } => {
                if let Some((wx, wy)) = self.vista.tela_para_monitor(x, y) {
                    let (fx, fy) = fio_para_fracao(wx, wy);
                    self.mover_abs(fx, fy, out);
                }
            }
            Acao::Clique(b) => {
                for down in [true, false] {
                    out.push(Msg::MouseButton(MouseButton {
                        button: botao(b).into(),
                        down,
                    }));
                }
            }
            Acao::Pressionar(b) | Acao::Soltar(b) => out.push(Msg::MouseButton(MouseButton {
                button: botao(b).into(),
                down: matches!(a, Acao::Pressionar(_)),
            })),
            Acao::Rolar { dx, dy } => {
                // Pixels da tela do celular → pixels lógicos do monitor (com zoom, rola menos).
                let img = self.vista.imagem();
                let monitor = self
                    .monitores
                    .atual()
                    .map_or((1920.0, 1080.0), |d| (d.width as f32, d.height as f32));
                let (sx, sy) = (monitor.0 / img.w.max(1.0), monitor.1 / img.h.max(1.0));
                let (rx, ry) = ((dx * sx).round() as i32, (dy * sy).round() as i32);
                if rx != 0 || ry != 0 {
                    out.push(Msg::Scroll(Scroll {
                        dx: rx,
                        dy: ry,
                        precise: true,
                    }));
                }
            }
            Acao::FimRolagem => out.push(Msg::Scroll(Scroll {
                dx: 0,
                dy: 0,
                precise: true,
            })),
            Acao::Zoom { fator, cx, cy } => self.vista.zoom(fator, cx, cy),
            Acao::TrocarMonitor(passo) => {
                self.monitores.trocar(passo, self.prefs, out);
                self.ajustar_vista();
            }
        }
    }

    /// Ações que vieram de fora da máquina de gestos (a pinça do Slint).
    pub fn aplicar(&mut self, acoes: Vec<Acao>, out: &mut Vec<Msg>) {
        for a in acoes {
            self.acao(a, out);
        }
    }

    /// Um evento de dedo na área do vídeo.
    pub fn toque(&mut self, t: Toque, out: &mut Vec<Msg>) {
        let mut acoes = std::mem::take(&mut self.acoes);
        self.gestos.toque(t, &mut acoes);
        for a in acoes.drain(..) {
            self.acao(a, out);
        }
        self.acoes = acoes;
    }

    /// Relógio (segurar sem mexer).
    pub fn tick(&mut self, t_ms: u64, out: &mut Vec<Msg>) {
        let mut acoes = std::mem::take(&mut self.acoes);
        self.gestos.tick(t_ms, &mut acoes);
        for a in acoes.drain(..) {
            self.acao(a, out);
        }
        self.acoes = acoes;
    }

    fn enviar_teclas(&mut self, out: &mut Vec<Msg>) {
        out.extend(self.teclas.drain(..).map(|k| k.to_proto()));
    }

    /// Texto do teclado do sistema (IME já resolvido).
    pub fn texto(&mut self, s: &str, out: &mut Vec<Msg>) {
        self.teclado.texto(s, &mut self.teclas);
        self.enviar_teclas(out);
    }

    /// Tecla da faixa.
    pub fn tecla(&mut self, t: TeclaFaixa, out: &mut Vec<Msg>) {
        self.teclado.tecla(t, &mut self.teclas);
        self.enviar_teclas(out);
    }

    /// Apagar pedido pelo teclado do sistema.
    pub fn apagar(&mut self, n: usize, out: &mut Vec<Msg>) {
        self.teclado.apagar(n, &mut self.teclas);
        self.enviar_teclas(out);
    }

    /// Toque num modificador da faixa (um toque = uma vez; dois = travado).
    pub fn modificador(&mut self, m: Mods) {
        self.teclado.tocar(m);
    }

    /// Toque longo num modificador: trava direto (mockup 08).
    pub fn travar_modificador(&mut self, m: Mods) {
        while self.teclado.trava(m) != Trava::Travada {
            self.teclado.tocar(m);
        }
    }

    /// Ctrl+Alt+Del: a sequência de atenção segura quando o host atende; senão as teclas (o
    /// Linux recebe como atalho comum).
    pub fn ctrl_alt_del(&mut self, out: &mut Vec<Msg>) {
        if self.atencao_segura {
            out.push(Msg::SecureAttention(SecureAttention {}));
            return;
        }
        let antes = self.teclado.clone();
        self.teclado = VirtualKeyboard::default();
        self.teclado.tocar(Mods::CTRL);
        self.teclado.tocar(Mods::ALT);
        self.teclado.tecla(TeclaFaixa::Delete, &mut self.teclas);
        self.teclado = antes;
        self.enviar_teclas(out);
    }

    /// Saiu da sessão, foi para segundo plano ou a tela girou no meio de um gesto: solta tudo
    /// no host (MF-50).
    pub fn soltar_tudo(&mut self, out: &mut Vec<Msg>) {
        let mut acoes = std::mem::take(&mut self.acoes);
        self.gestos.cancelar(&mut acoes);
        for a in acoes.drain(..) {
            self.acao(a, out);
        }
        self.acoes = acoes;
        self.teclado = VirtualKeyboard::default();
        out.push(Msg::ReleaseAll(ReleaseAll {}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gestos::Fase;
    use taildesk_proto::control::Rotation;

    fn monitor(id: u32, primary: bool) -> DisplayInfo {
        DisplayInfo {
            id,
            name: format!("M{id}"),
            width: 1920,
            height: 1080,
            primary,
            rotation: Rotation::Normal.into(),
            shared: true,
            ..DisplayInfo::default()
        }
    }

    fn area() -> Retangulo {
        Retangulo {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        }
    }

    fn controle(modo: Modo) -> (Controle, Vec<Msg>) {
        let mut c = Controle::new(modo, 1.0, area());
        let mut out = Vec::new();
        c.monitores_mudaram(&[monitor(3, false), monitor(7, true)], &mut out);
        (c, out)
    }

    fn toque(c: &mut Controle, id: i32, fase: Fase, x: f32, y: f32, t: u64, out: &mut Vec<Msg>) {
        c.toque(
            Toque {
                id,
                fase,
                x,
                y,
                t_ms: t,
            },
            out,
        );
    }

    #[test]
    fn hello_de_celular() {
        let h = hello(&Aparelho {
            modelo: "Pixel 8".into(),
            arch: "aarch64".into(),
            tablet: false,
            largura_px: 2400,
            altura_px: 1080,
            dpi: 420,
            decode_hw: true,
            idioma_teclado: "pt-BR".into(),
            versao_app: "0.3.0".into(),
        });
        assert_eq!(h.client_kind(), ClientKind::Phone);
        assert_eq!(h.viewport.unwrap().w_px, 2400);
        assert_eq!(
            h.keyboard.as_ref().unwrap().source(),
            KeyboardSource::Virtual
        );
        assert!(h.decode_caps[0].hw);
        // Passa nos limites do parser.
        let ctl = taildesk_proto::control::Control::from(Msg::Hello(h));
        assert!(ctl.validate().is_ok());
    }

    #[test]
    fn comeca_no_principal_e_troca_um_por_vez() {
        let (mut c, out) = controle(Modo::Touchpad);
        assert_eq!(
            out,
            vec![Msg::StartVideo(StartVideo {
                display_id: 7,
                wanted: None,
                priority: Priority::Focused.into(),
            })]
        );
        assert_eq!(c.monitores.posicao(), (2, 2));
        let mut out = Vec::new();
        c.monitores.trocar(1, None, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], Msg::StopVideo(StopVideo { display_id: 7 }));
        assert!(matches!(&out[1], Msg::StartVideo(s) if s.display_id == 3));
        // Sumiu o monitor atual: vai para o que sobrou sem StopVideo (o host já parou).
        let mut out = Vec::new();
        c.monitores_mudaram(&[monitor(7, true)], &mut out);
        assert!(matches!(&out[..], [Msg::StartVideo(s)] if s.display_id == 7));
        // Um só: trocar não faz nada.
        let mut out = Vec::new();
        c.monitores.trocar(1, None, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn toque_direto_clica_onde_tocou() {
        let (mut c, _) = controle(Modo::ToqueDireto);
        let mut out = Vec::new();
        toque(&mut c, 0, Fase::Desceu, 960.0, 540.0, 0, &mut out);
        toque(&mut c, 0, Fase::Subiu, 960.0, 540.0, 50, &mut out);
        assert_eq!(out.len(), 3, "{out:?}");
        let Msg::MouseMove(MouseMove {
            pos: Some(mouse_move::Pos::Abs(p)),
        }) = &out[0]
        else {
            panic!("{out:?}")
        };
        assert_eq!((p.display_id, p.x, p.y), (7, 32_768, 32_768));
        assert!(matches!(
            out[1],
            Msg::MouseButton(MouseButton { down: true, .. })
        ));
        assert!(matches!(
            out[2],
            Msg::MouseButton(MouseButton { down: false, .. })
        ));
    }

    #[test]
    fn touchpad_move_a_partir_do_cursor_do_host() {
        let (mut c, _) = controle(Modo::Touchpad);
        c.cursor_do_host(&CursorPos {
            display_id: 7,
            x: 0,
            y: 0,
            visible: true,
        });
        let mut out = Vec::new();
        toque(&mut c, 0, Fase::Desceu, 100.0, 100.0, 0, &mut out);
        toque(&mut c, 0, Fase::Moveu, 200.0, 100.0, 100, &mut out);
        let Some(Msg::MouseMove(MouseMove {
            pos: Some(mouse_move::Pos::Abs(p)),
        })) = out.last()
        else {
            panic!("{out:?}")
        };
        assert!(p.x > 0 && p.y == 0, "{p:?}");
        // Cursor de outro monitor não mexe no nosso.
        c.cursor_do_host(&CursorPos {
            display_id: 3,
            x: 65_535,
            y: 65_535,
            visible: true,
        });
        assert!(c.cursor().0 < 0.5);
    }

    #[test]
    fn rolagem_vai_em_pixels_precisos_e_termina() {
        let (mut c, _) = controle(Modo::Touchpad);
        let mut out = Vec::new();
        toque(&mut c, 0, Fase::Desceu, 100.0, 500.0, 0, &mut out);
        toque(&mut c, 1, Fase::Desceu, 200.0, 500.0, 5, &mut out);
        toque(&mut c, 0, Fase::Moveu, 100.0, 450.0, 20, &mut out);
        toque(&mut c, 1, Fase::Moveu, 200.0, 450.0, 21, &mut out);
        toque(&mut c, 0, Fase::Subiu, 100.0, 450.0, 40, &mut out);
        toque(&mut c, 1, Fase::Subiu, 200.0, 450.0, 41, &mut out);
        let rolagens: Vec<&Scroll> = out
            .iter()
            .filter_map(|m| match m {
                Msg::Scroll(s) => Some(s),
                _ => None,
            })
            .collect();
        assert!(rolagens.iter().all(|s| s.precise));
        assert_eq!(rolagens.iter().map(|s| s.dy).sum::<i32>(), 50);
        assert_eq!(rolagens.last().map(|s| (s.dx, s.dy)), Some((0, 0)));
    }

    #[test]
    fn ctrl_alt_del_e_soltar_tudo() {
        let (mut c, _) = controle(Modo::Touchpad);
        let mut out = Vec::new();
        c.atencao_segura = true;
        c.ctrl_alt_del(&mut out);
        assert_eq!(out, vec![Msg::SecureAttention(SecureAttention {})]);
        c.atencao_segura = false;
        out.clear();
        c.modificador(Mods::SHIFT);
        c.ctrl_alt_del(&mut out);
        assert_eq!(out.len(), 6, "Ctrl, Alt, Del e as soltas: {out:?}");
        // O Shift de antes continua esperando a próxima tecla.
        assert_eq!(c.teclado.trava(Mods::SHIFT), Trava::UmaVez);

        out.clear();
        c.travar_modificador(Mods::CTRL);
        toque(&mut c, 0, Fase::Desceu, 100.0, 100.0, 0, &mut out);
        c.tick(700, &mut out);
        out.clear();
        c.soltar_tudo(&mut out);
        assert_eq!(
            out,
            vec![
                Msg::MouseButton(MouseButton {
                    button: Button::Left.into(),
                    down: false
                }),
                Msg::ReleaseAll(ReleaseAll {}),
            ]
        );
        assert_eq!(c.teclado.mods(), Mods::NONE);
    }

    #[test]
    fn texto_e_atalho_pelo_controle() {
        let (mut c, _) = controle(Modo::Touchpad);
        let mut out = Vec::new();
        c.texto("ação", &mut out);
        assert!(matches!(&out[..], [Msg::KeyChar(k)] if k.text == "ação"));
        out.clear();
        c.modificador(Mods::CTRL);
        c.texto("c", &mut out);
        assert!(
            matches!(&out[0], Msg::KeyShortcut(k) if k.base_char == "c" && k.mods == 2 && k.down)
        );
    }
}
