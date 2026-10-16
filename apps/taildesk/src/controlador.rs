//! Este computador como controlador: "Conectar" abre a janela da sessão (`12-levantamento-ux.md`
//! §5.3 e §5.6), com as etapas de conexão, abas dos monitores, mouse sobre o vídeo, teclado
//! traduzido, o vídeo do monitor ativo e o cursor do host desenhado por cima do vídeo.

use std::cell::RefCell;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::platform::PointerEventButton;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{
    CloseRequestResponse, ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel,
};
use taildesk_codec::{CodecVideo, QuadroYuv};
use taildesk_keyboard::{Controller, Hid, KeyMsg, Keymap, Level, Mode, Mods, Translation};
use taildesk_media::video::RecepcaoVideo;
use taildesk_overlay::{Overlay, SystemOverlay};
use taildesk_proto::control::{
    AbsPos, Button, Codec, CodecCap, CodecSelected, Control, Feature, Hello, KeyboardInfo,
    MouseButton, MouseMove, Msg, Platform, RequestKeyframe, Scroll, mouse_move,
};
use taildesk_session::conexao::PORTA;
use taildesk_session::conexao::controlador::{EventoControlador, Sessao};
use taildesk_session::{ControllerState, CursorControlador, EndReason, UmPorVez, atalho};
use taildesk_transport::{Pin, client_endpoint};
use taildesk_tsapi::LocalApi;
use taildesk_ui::{AbaMonitor, JanelaSessao};
use tokio::sync::{mpsc, oneshot};

use crate::video::{Contadores, DecoderMedido, Recepcoes, Tela, ler_datagramas, travar};
use crate::{config, no_app};

mod arquivos;
mod diagnostico;

use diagnostico::{Diagnostico, Rede};

/// Atualização do painel de diagnóstico.
const INTERVALO_DIAGNOSTICO: Duration = Duration::from_secs(1);
/// Rota do Tailscale (pede um `ping` disco ao `tailscaled`, então menos vezes).
const INTERVALO_ROTA: Duration = Duration::from_secs(10);

type IdLocal = u64;

/// Layout deste computador (o controlador traduz a partir dele).
fn teclado_local() -> (Keymap, String) {
    #[cfg(windows)]
    {
        let k = taildesk_platform::teclado::windows::ler_keymap_ativo();
        let id = k.name.clone();
        (k, id)
    }
    #[cfg(target_os = "linux")]
    {
        use taildesk_platform::teclado::xkb;
        let (layout, variante) =
            xkb::layout_da_sessao().unwrap_or_else(|| ("us".into(), String::new()));
        match xkb::ler_keymap_nomes(&layout, &variante, &xkb::locale_da_sessao()) {
            Ok(k) => {
                let id = k.name.clone();
                (k, id)
            }
            Err(_) => (Keymap::default(), String::new()),
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        (Keymap::default(), String::new())
    }
}

/// Código físico do winit → HID. No Windows o winit dá o scancode (com 0xE0 nos estendidos);
/// no Linux, o código evdev.
fn hid_de(tecla: winit::keyboard::PhysicalKey) -> Option<Hid> {
    use winit::platform::scancode::PhysicalKeyExtScancode;
    let codigo = u16::try_from(tecla.to_scancode()?).ok()?;
    if cfg!(windows) {
        taildesk_keyboard::codes::scancode_to_hid(codigo)
    } else {
        taildesk_keyboard::codes::evdev_to_hid(codigo)
    }
}

fn texto_fim(r: EndReason, host: &str) -> String {
    match r {
        EndReason::Refused => format!("{host} recusou o acesso."),
        EndReason::NoAnswer => format!("Ninguém respondeu em {host}."),
        EndReason::NotAllowed => format!("{host} não aceita conexões deste computador."),
        EndReason::VersionMismatch => format!("{host} usa outra versão do TailDesk."),
        EndReason::ByHost => format!("{host} encerrou a sessão."),
        EndReason::ByUser => "Você encerrou a sessão.".into(),
        EndReason::ProtocolError => format!("{host} mandou algo inesperado; sessão encerrada."),
        EndReason::Lost => format!("A conexão com {host} caiu."),
    }
}

/// Uma sessão aberta numa janela.
struct SessaoUi {
    id: IdLocal,
    janela: JanelaSessao,
    host: String,
    saida: Option<mpsc::Sender<Control>>,
    fechar: Option<oneshot::Sender<()>>,
    estado: ControllerState,
    monitores: UmPorVez,
    /// Monitores do `HelloAck` (o vídeo só é pedido depois de liberado).
    displays: Vec<taildesk_proto::control::DisplayInfo>,
    cursor: CursorControlador,
    /// Imagem de cada forma de cursor já convertida (hash → imagem).
    imagens_cursor: HashMap<u64, Image>,
    teclado: Controller,
    tag_local: String,
    /// AltGr existe no layout local (então o Alt direito é AltGr).
    tem_altgr: bool,
    mods_presos: Vec<Hid>,
    /// Último ponto do mouse sobre o vídeo, para desenhar o cursor sem esperar o host.
    local: Option<(u32, u16, u16)>,
    botoes: u8,
    painel: arquivos::Painel,
    /// Recepção e decode do monitor ativo (a task dos datagramas entrega aqui).
    recepcoes: Recepcoes,
    tela: Tela,
    /// Monitor do vídeo que a janela mostra agora.
    video_display: Option<u32>,
    contadores: Arc<Contadores>,
    diagnostico: Diagnostico,
}

impl SessaoUi {
    fn enviar(&self, msg: Msg) {
        if let Msg::StopVideo(v) = &msg {
            self.parar_video(v.display_id);
        }
        if let Some(s) = &self.saida {
            let _ = s.try_send(Control { msg: Some(msg) });
        }
    }

    fn enviar_todas(&self, msgs: Vec<Msg>) {
        for m in msgs {
            self.enviar(m);
        }
    }

    /// `CodecSelected`: começa a receber e decodificar o vídeo desse monitor.
    fn iniciar_video(&mut self, c: &CodecSelected) {
        let Ok(stream) = u8::try_from(c.stream_id) else {
            return;
        };
        let display = c.display_id;
        let mut falhas = Vec::new();
        let codec = match Codec::try_from(c.codec) {
            Ok(Codec::Hevc) => CodecVideo::Hevc,
            Ok(Codec::Av1) => CodecVideo::Av1,
            _ => CodecVideo::H264,
        };
        let decoder = taildesk_codec::abrir_decoder(codec, &mut falhas);
        for (opcao, e) in falhas {
            eprintln!(
                "taildesk: decoder {} ({}) indisponível: {e}",
                opcao.backend,
                opcao.gpu.as_deref().unwrap_or("CPU")
            );
        }
        let decoder = match decoder {
            Ok(d) => DecoderMedido::novo(d, self.contadores.clone()),
            Err(e) => {
                self.janela
                    .set_etapa(format!("Não foi possível decodificar o vídeo: {e}").into());
                return;
            }
        };
        self.diagnostico.video(c);
        let (caixa, id) = (self.tela.caixa(), self.id);
        let ao_quadro = Box::new(move |_: u32, q: &QuadroYuv<'_>| {
            if caixa.guardar(q) {
                na_sessao(id, SessaoUi::mostrar_quadro);
            }
        });
        let saida = self.saida.clone();
        let contadores = self.contadores.clone();
        let pedir_chave = Arc::new(move || {
            contadores.pedidos_chave.fetch_add(1, Ordering::Relaxed);
            if let Some(s) = &saida {
                let _ = s.try_send(Control {
                    msg: Some(Msg::RequestKeyframe(RequestKeyframe {
                        display_id: display,
                    })),
                });
            }
        });
        match RecepcaoVideo::iniciar(display, stream, Box::new(decoder), ao_quadro, pedir_chave) {
            Ok(r) => {
                let mut m = travar(&self.recepcoes);
                m.retain(|_, (d, _)| *d != display);
                m.insert(stream, (display, r));
            }
            Err(e) => eprintln!("taildesk: vídeo: {e}"),
        }
        if self.video_display != Some(display) {
            // Outro monitor: não deixa o quadro do anterior na tela.
            self.janela.set_tem_video(false);
            self.video_display = Some(display);
        }
    }

    fn parar_video(&self, display: u32) {
        travar(&self.recepcoes).retain(|_, (d, _)| *d != display);
    }

    /// Chamado pelo decode (pela fila da interface) quando há quadro novo.
    fn mostrar_quadro(&mut self) {
        if let Some(img) = self.tela.proxima_imagem() {
            self.contadores.exibidos.fetch_add(1, Ordering::Relaxed);
            self.janela.set_quadro(img);
            self.janela.set_tem_video(true);
        }
    }

    /// Recalcula o painel de diagnóstico (uma vez por [`INTERVALO_DIAGNOSTICO`]).
    fn atualizar_diagnostico(&mut self, rede: Rede) {
        self.diagnostico.rede(rede);
        let linhas = self.diagnostico.linhas(&self.contadores);
        if self.janela.get_diagnostico_aberto() {
            self.janela
                .set_diagnostico(ModelRc::new(VecModel::from(linhas)));
        }
    }

    fn ao_vivo(&self) -> bool {
        matches!(self.estado, ControllerState::Live { .. })
    }

    fn controla(&self) -> bool {
        matches!(self.estado, ControllerState::Live { permissions } if permissions.control)
    }

    fn mods(&self) -> Mods {
        let mut m = Mods::NONE;
        for &h in &self.mods_presos {
            m = m.with(match h {
                0xE0 | 0xE4 => Mods::CTRL,
                0xE1 | 0xE5 => Mods::SHIFT,
                0xE2 => Mods::ALT,
                0xE6 if self.tem_altgr => Mods::ALTGR,
                0xE6 => Mods::ALT,
                0xE3 | 0xE7 => Mods::META,
                _ => Mods::NONE,
            });
        }
        m
    }

    fn abas(&self) {
        let abas: Vec<AbaMonitor> = if self.monitores.mostrar_abas() {
            self.monitores
                .abas()
                .into_iter()
                .map(|a| AbaMonitor {
                    texto: format!(
                        "{} · {}{}",
                        a.numero,
                        a.rotulo,
                        if a.principal { " ★" } else { "" }
                    )
                    .into(),
                    ativa: a.ativa,
                    liberada: a.liberado,
                })
                .collect()
        } else {
            Vec::new()
        };
        self.janela.set_abas(ModelRc::new(VecModel::from(abas)));
    }

    /// Redesenha o cursor do host em cima do vídeo.
    fn desenhar_cursor(&mut self) {
        let local = if self.controla() { self.local } else { None };
        let Some(d) = self.cursor.desenho(local) else {
            self.janela.set_cursor_visivel(false);
            return;
        };
        let Some(m) = self.monitores.monitor_ativo() else {
            self.janela.set_cursor_visivel(false);
            return;
        };
        if d.display_id != m.id {
            self.janela.set_cursor_visivel(false);
            return;
        }
        let (aw, ah) = (
            self.janela.get_area_largura(),
            self.janela.get_area_altura(),
        );
        let (iw, ih) = (m.width.max(1) as f32, m.height.max(1) as f32);
        let escala = (aw / iw).min(ah / ih);
        let (ox, oy) = ((aw - iw * escala) / 2.0, (ah - ih * escala) / 2.0);
        let px = ox + f32::from(d.x) / 65535.0 * iw * escala;
        let py = oy + f32::from(d.y) / 65535.0 * ih * escala;
        let (hx, hy, imagem) = match d.forma {
            Some(f) => {
                let h = f.header;
                let img = self
                    .imagens_cursor
                    .entry(h.hash)
                    .or_insert_with(|| {
                        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                            &f.pixels,
                            u32::from(h.width),
                            u32::from(h.height),
                        );
                        Image::from_rgba8(buf)
                    })
                    .clone();
                (f32::from(h.hot_x), f32::from(h.hot_y), img)
            }
            None => (0.0, 0.0, seta()),
        };
        self.janela.set_cursor(imagem);
        self.janela.set_cursor_x(px - hx);
        self.janela.set_cursor_y(py - hy);
        self.janela.set_cursor_visivel(true);
    }

    fn evento(&mut self, ev: EventoControlador) {
        let mut out = Vec::new();
        match ev {
            EventoControlador::Estado(s) => {
                let antes = self.ao_vivo();
                self.estado = s;
                match s {
                    ControllerState::Connecting => self
                        .janela
                        .set_etapa(format!("Encontrando {}…", self.host).into()),
                    ControllerState::AwaitingConsent { .. } => self
                        .janela
                        .set_etapa(format!("Esperando {} aceitar…", self.host).into()),
                    ControllerState::Live { .. } => {
                        self.janela.set_ao_vivo(true);
                        if !antes {
                            // Pede o vídeo do monitor principal, agora que pode.
                            self.monitores = UmPorVez::new();
                            self.monitores.atualizar(&self.displays, &mut out);
                        }
                    }
                    ControllerState::Ended(r) => {
                        self.janela.set_ao_vivo(false);
                        self.janela.set_terminou(true);
                        self.janela.set_etapa(texto_fim(r, &self.host).into());
                        self.saida = None;
                        travar(&self.recepcoes).clear();
                    }
                }
            }
            EventoControlador::Ack(a) => {
                if !a.host_name.is_empty() {
                    self.host = a.host_name.clone();
                    self.janela.set_host(self.host.as_str().into());
                }
                let remoto = a.keyboard.clone().unwrap_or_default();
                let t = taildesk_keyboard::effective_translation(
                    Mode::Auto,
                    &self.tag_local,
                    &remoto.layout_tag,
                );
                let mut km = Vec::new();
                self.teclado.set_translation(t, &mut km);
                let local = self.tag_local.clone();
                let remoto_txt = if remoto.layout_tag.is_empty() {
                    "?".to_owned()
                } else {
                    remoto.layout_tag.clone()
                };
                self.janela.set_teclado(
                    format!(
                        "⌨ {local} → {remoto_txt}{}",
                        if t == Translation::Physical {
                            " (físico)"
                        } else {
                            ""
                        }
                    )
                    .into(),
                );
                // Antes de ao vivo só guarda a lista; o `StartVideo` sai quando liberar.
                self.displays.clone_from(&a.displays);
                let mut descartar = Vec::new();
                self.monitores.atualizar(&a.displays, &mut descartar);
                if self.ao_vivo() {
                    out = descartar;
                }
            }
            EventoControlador::Forma(f) => {
                self.cursor.forma_recebida(f);
            }
            EventoControlador::Arquivos(e) => self.painel.evento(&self.janela, e),
            EventoControlador::Mensagem(m) => match *m {
                Msg::DisplaysChanged(d) => {
                    self.displays.clone_from(&d.displays);
                    self.monitores.atualizar(&d.displays, &mut out);
                }
                Msg::CursorPos(p) => {
                    self.monitores.on_cursor(&p, &mut out);
                    self.cursor.on_pos(p);
                }
                Msg::CursorShapeRef(r) => self.cursor.on_shape_ref(r),
                Msg::CodecSelected(c) => self.iniciar_video(&c),
                Msg::VideoStats(v) => self.diagnostico.host(v),
                Msg::VideoStopped(v) => {
                    self.parar_video(v.display_id);
                    self.monitores.on_video_stopped(&v, &mut out);
                }
                _ => {}
            },
        }
        self.enviar_todas(out);
        self.abas();
        self.desenhar_cursor();
        self.painel.estado(&self.janela, &self.estado);
    }

    fn tecla(&mut self, hid: Hid, down: bool) -> bool {
        if taildesk_keyboard::is_modifier(hid) {
            self.mods_presos.retain(|&h| h != hid);
            if down {
                self.mods_presos.push(hid);
            }
        }
        if down && let Some(a) = atalho(hid, self.mods()) {
            let mut out = Vec::new();
            if self.monitores.aplicar(a, &mut out) {
                self.enviar_todas(out);
                self.abas();
            }
            return true;
        }
        if !self.controla() {
            return false;
        }
        let mut km = Vec::new();
        let mods = self.mods();
        self.teclado.key(hid, down, mods, &mut km);
        for k in km {
            self.enviar(k.to_proto());
        }
        true
    }

    fn foco_perdido(&mut self) {
        self.mods_presos.clear();
        if self.controla() {
            let mut km = Vec::new();
            self.teclado.focus_lost(&mut km);
            for k in km {
                self.enviar(k.to_proto());
            }
            self.enviar(KeyMsg::ReleaseAll.to_proto());
        }
    }

    fn mouse_moveu(&mut self, x: f32, y: f32) {
        let area = (
            self.janela.get_area_largura(),
            self.janela.get_area_altura(),
        );
        let ponto = self.monitores.ponto(area, (x, y), self.botoes != 0);
        self.local = ponto;
        if let Some((display_id, x, y)) = ponto
            && self.controla()
        {
            self.enviar(Msg::MouseMove(MouseMove {
                pos: Some(mouse_move::Pos::Abs(AbsPos {
                    display_id,
                    x: x.into(),
                    y: y.into(),
                })),
            }));
        }
        self.desenhar_cursor();
    }

    fn mouse_botao(&mut self, b: PointerEventButton, down: bool, x: f32, y: f32) {
        let botao = match b {
            PointerEventButton::Left => Button::Left,
            PointerEventButton::Right => Button::Right,
            PointerEventButton::Middle => Button::Middle,
            PointerEventButton::Back => Button::Back,
            PointerEventButton::Forward => Button::Forward,
            _ => return,
        };
        let bit = 1u8 << (botao as i32);
        if down {
            self.botoes |= bit;
        } else {
            self.botoes &= !bit;
        }
        self.mouse_moveu(x, y);
        if self.controla() && (self.local.is_some() || !down) {
            self.enviar(Msg::MouseButton(MouseButton {
                button: botao.into(),
                down,
            }));
        }
    }

    fn mouse_rolou(&mut self, dx: f32, dy: f32) {
        if self.controla() && self.local.is_some() {
            // Slint: positivo = para cima/esquerda; no fio, positivo = para baixo/direita.
            self.enviar(Msg::Scroll(Scroll {
                dx: -dx.round() as i32,
                dy: -dy.round() as i32,
                precise: true,
            }));
        }
    }
}

/// Seta padrão enquanto a forma do host não chega.
fn seta() -> Image {
    thread_local! {
        static SETA: Image = {
            const L: u32 = 12;
            const A: u32 = 19;
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(L, A);
            let px = buf.make_mut_slice();
            for y in 0..A {
                for x in 0..L {
                    let dentro = x <= y && x + y / 3 < 12 && y < 17;
                    let borda = dentro && (x == 0 || x == y || x + y / 3 == 11 || y == 16);
                    px[(y * L + x) as usize] = if borda {
                        Rgba8Pixel { r: 0, g: 0, b: 0, a: 255 }
                    } else if dentro {
                        Rgba8Pixel { r: 255, g: 255, b: 255, a: 255 }
                    } else {
                        Rgba8Pixel { r: 0, g: 0, b: 0, a: 0 }
                    };
                }
            }
            Image::from_rgba8(buf)
        };
    }
    SETA.with(Image::clone)
}

/// Sessões abertas neste controlador.
#[derive(Default)]
pub struct Controlador {
    sessoes: RefCell<HashMap<IdLocal, SessaoUi>>,
    proxima: std::cell::Cell<IdLocal>,
}

impl std::fmt::Debug for Controlador {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controlador").finish_non_exhaustive()
    }
}

/// Roda `f` na sessão `id`, na thread da interface.
fn na_sessao(id: IdLocal, f: impl FnOnce(&mut SessaoUi) + Send + 'static) {
    no_app(move |app| {
        if let Some(s) = app.controlador.sessoes.borrow_mut().get_mut(&id) {
            f(s);
        }
    });
}

/// Igual a [`na_sessao`], já na thread da interface (callbacks do Slint e do winit).
fn na_sessao_ja<R>(id: IdLocal, f: impl FnOnce(&mut SessaoUi) -> R) -> Option<R> {
    crate::com_app(|app| app.controlador.sessoes.borrow_mut().get_mut(&id).map(f)).flatten()
}

impl Controlador {
    /// Alguma janela de sessão aberta.
    pub fn ativa(&self) -> bool {
        !self.sessoes.borrow().is_empty()
    }

    /// "Conectar" num dispositivo da lista.
    pub fn conectar(&self, rt: &tokio::runtime::Handle, nome: String, ip: String) {
        let Ok(ip) = ip.parse::<IpAddr>() else {
            crate::avisar(format!("{nome} não tem IP na tailnet."));
            return;
        };
        let janela = match JanelaSessao::new() {
            Ok(j) => j,
            Err(e) => {
                crate::avisar(format!("Não foi possível abrir a janela da sessão: {e}"));
                return;
            }
        };
        let id = self.proxima.get() + 1;
        self.proxima.set(id);
        janela.set_host(nome.as_str().into());
        janela.set_etapa(format!("Encontrando {nome}…").into());

        janela.on_desconectar(move || fechar(id));
        janela.window().on_close_requested(move || {
            fechar(id);
            CloseRequestResponse::HideWindow
        });
        janela.on_trocar_aba(move |i| {
            na_sessao_ja(id, |s| {
                let abas = s.monitores.abas();
                if let Some(a) = abas.get(i as usize) {
                    let mut out = Vec::new();
                    if s.monitores.trocar(a.display_id, &mut out) {
                        s.enviar_todas(out);
                        s.abas();
                    }
                }
            });
        });
        janela.on_mouse_moveu(move |x, y| {
            na_sessao_ja(id, |s| s.mouse_moveu(x, y));
        });
        janela.on_mouse_botao(move |b, down, x, y| {
            na_sessao_ja(id, |s| s.mouse_botao(b, down, x, y));
        });
        janela.on_mouse_rolou(move |dx, dy| {
            na_sessao_ja(id, |s| s.mouse_rolou(dx, dy));
        });
        arquivos::ligar_callbacks(&janela, id);
        // Teclado pelo código físico, antes do Slint (Tab, Alt e atalhos vão para o host).
        janela
            .window()
            .on_winit_window_event(move |_, ev| match ev {
                winit::event::WindowEvent::KeyboardInput {
                    event,
                    is_synthetic: false,
                    ..
                } => {
                    let Some(hid) = hid_de(event.physical_key) else {
                        return EventResult::Propagate;
                    };
                    let down = event.state.is_pressed();
                    let consumiu = na_sessao_ja(id, |s| s.tecla(hid, down)).unwrap_or(false);
                    if consumiu {
                        EventResult::PreventDefault
                    } else {
                        EventResult::Propagate
                    }
                }
                winit::event::WindowEvent::Focused(false) => {
                    na_sessao_ja(id, SessaoUi::foco_perdido);
                    EventResult::Propagate
                }
                // Arrastar arquivos do sistema (Windows e X11; o Wayland vem por `ligar_soltar`).
                winit::event::WindowEvent::HoveredFile(_) => {
                    na_sessao_ja(id, |s| s.painel.arrastando(&s.janela, true));
                    EventResult::Propagate
                }
                winit::event::WindowEvent::HoveredFileCancelled => {
                    na_sessao_ja(id, |s| s.painel.arrastando(&s.janela, false));
                    EventResult::Propagate
                }
                winit::event::WindowEvent::DroppedFile(p) => {
                    let p = p.clone();
                    na_sessao_ja(id, |s| s.painel.soltou(&s.janela, vec![p]));
                    EventResult::Propagate
                }
                _ => EventResult::Propagate,
            });
        if let Err(e) = janela.show() {
            crate::avisar(format!("Não foi possível abrir a janela da sessão: {e}"));
            return;
        }

        let (keymap, os_layout_id) = teclado_local();
        let tag_local = taildesk_keyboard::layout_tag(&os_layout_id);
        let tem_altgr = keymap
            .keys
            .iter()
            .any(|(_, l, _)| matches!(l, Level::AltGr | Level::ShiftAltGr));
        let info = KeyboardInfo {
            layout_tag: tag_local.clone(),
            os_layout_id,
            extra_hid: Vec::new(),
        };
        let (fechar_tx, fechar_rx) = oneshot::channel();
        let recepcoes = Recepcoes::default();
        let contadores = Arc::new(Contadores::default());
        self.sessoes.borrow_mut().insert(
            id,
            SessaoUi {
                id,
                janela,
                host: nome.clone(),
                saida: None,
                fechar: Some(fechar_tx),
                estado: ControllerState::Connecting,
                monitores: UmPorVez::new(),
                displays: Vec::new(),
                cursor: CursorControlador::new(),
                imagens_cursor: HashMap::new(),
                teclado: Controller::new(keymap, Translation::Hybrid),
                tag_local,
                tem_altgr,
                mods_presos: Vec::new(),
                local: None,
                botoes: 0,
                painel: arquivos::Painel::new(id, rt.clone()),
                recepcoes: recepcoes.clone(),
                tela: Tela::default(),
                video_display: None,
                contadores: contadores.clone(),
                diagnostico: Diagnostico::default(),
            },
        );
        arquivos::ligar_soltar(id, 50);
        rt.spawn(rodar(id, nome, ip, info, recepcoes, contadores, fechar_rx));
    }
}

/// Desconectar (ou cancelar a espera) e fechar a janela.
fn fechar(id: IdLocal) {
    no_app(move |app| {
        if let Some(mut s) = app.controlador.sessoes.borrow_mut().remove(&id) {
            s.foco_perdido();
            if let Some(f) = s.fechar.take() {
                let _ = f.send(());
            }
            let _ = s.janela.hide();
        }
    });
}

/// Conecta e repassa os eventos à janela até a sessão acabar ou ser fechada.
async fn rodar(
    id: IdLocal,
    nome: String,
    ip: IpAddr,
    teclado: KeyboardInfo,
    recepcoes: Recepcoes,
    contadores: Arc<Contadores>,
    mut fechar_rx: oneshot::Receiver<()>,
) {
    let remoto = SocketAddr::new(ip, PORTA);
    // Quem é o host na tailnet: o fingerprint fica ligado ao `node_stable_id` (TOFU).
    let overlay = SystemOverlay::new(LocalApi::default());
    let node = overlay
        .identify(remoto)
        .await
        .ok()
        .map(|p| p.node_stable_id);
    let conhecido = node
        .as_ref()
        .and_then(|n| config::ler().hosts.get(n).copied());
    let pin = conhecido.map_or(Pin::FirstContact, Pin::Sha256);
    // H.264 sempre; HEVC e AV1 quando há decoder de hardware (abre decoders para testar).
    let decode_caps = tokio::task::spawn_blocking(taildesk_codec::codecs_para_decodificar)
        .await
        .unwrap_or_else(|_| vec![CodecVideo::H264])
        .into_iter()
        .map(|c| CodecCap {
            codec: match c {
                CodecVideo::H264 => Codec::H264,
                CodecVideo::Hevc => Codec::Hevc,
                CodecVideo::Av1 => Codec::Av1,
            }
            .into(),
            ..CodecCap::default()
        })
        .collect();
    let hello = Hello {
        app_version: env!("CARGO_PKG_VERSION").into(),
        platform: Some(Platform {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            device_model: String::new(),
        }),
        features: vec![Feature::Files.into(), Feature::MultiDisplay.into()],
        decode_caps,
        keyboard: Some(teclado),
        ..Hello::default()
    };
    let falhou = |texto: String| {
        na_sessao(id, move |s| {
            s.janela.set_terminou(true);
            s.janela.set_etapa(texto.into());
        });
    };
    let endpoint = match client_endpoint(remoto) {
        Ok(e) => e,
        Err(e) => return falhou(format!("Não foi possível abrir a conexão: {e}")),
    };
    let conectar = Sessao::conectar(&endpoint, remoto, pin, hello, &nome);
    let (sessao, mut eventos) = tokio::select! {
        r = conectar => match r {
            Ok(v) => v,
            Err(e) if conhecido.is_some() => {
                return falhou(format!(
                    "Não foi possível conectar a {nome}. Se o TailDesk de lá foi reinstalado, \
                     a identidade dele mudou ({e})."
                ));
            }
            Err(e) => {
                return falhou(format!(
                    "Não foi possível conectar a {nome}. O TailDesk está aberto lá? ({e})"
                ));
            }
        },
        _ = &mut fechar_rx => return,
    };
    if conhecido.is_none()
        && let (Some(n), Some(fp)) = (node, sessao.fingerprint())
    {
        let mut c = config::ler();
        c.hosts.insert(n, fp);
        if let Err(e) = config::gravar(&c) {
            eprintln!("taildesk: não foi possível gravar o host conhecido: {e}");
        }
    }
    let saida = sessao.saida();
    let servico = sessao.arquivos().clone();
    na_sessao(id, move |s| {
        s.saida = Some(saida);
        s.painel.ligar(servico);
    });
    let conn = sessao.conexao().clone();
    let datagramas = tokio::spawn(ler_datagramas(conn.clone(), recepcoes, contadores));
    // Rota pelo Tailscale (direta ou DERP), em segundo plano: o `ping` disco pode demorar.
    let rota = Arc::new(Mutex::new(None));
    let tarefa_rota = tokio::spawn({
        let rota = rota.clone();
        async move {
            let mut t = tokio::time::interval(INTERVALO_ROTA);
            loop {
                t.tick().await;
                let p = overlay.path(ip).await;
                *rota
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(p);
            }
        }
    });
    let mut diag = tokio::time::interval(INTERVALO_DIAGNOSTICO);
    loop {
        tokio::select! {
            ev = eventos.recv() => match ev {
                Some(ev) => na_sessao(id, move |s| s.evento(ev)),
                None => break,
            },
            _ = diag.tick() => {
                let rede = Rede {
                    rtt: conn.rtt(),
                    rota: rota.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone(),
                };
                na_sessao(id, move |s| s.atualizar_diagnostico(rede));
            }
            _ = &mut fechar_rx => break,
        }
    }
    tarefa_rota.abort();
    datagramas.abort();
    sessao.encerrar().await;
    drop(endpoint);
}
