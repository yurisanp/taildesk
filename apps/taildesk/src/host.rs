//! Este computador como host: atende na porta do TailDesk no IP da tailnet, com a identidade
//! guardada no cofre do sistema, e mostra o pedido de acesso e a faixa de sessão ativa
//! (`12-levantamento-ux.md` §5.4 e §5.5).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{CloseRequestResponse, ComponentHandle, Timer, TimerMode};
use taildesk_keyboard::{Inject, Keymap, Locks};
use taildesk_overlay::{Overlay, SystemOverlay};
use taildesk_proto::control::{KeyboardInfo, Msg, Permissions, Platform};
use taildesk_session::conexao::PORTA;
use taildesk_session::conexao::host::{
    CanalMidia, ComandoHost, ConfigHost, Descricao, Entrada, EventoHost, IdSessao, Maquina, servir,
};
use taildesk_session::{ConsentAnswer, ConsentPrompt, HostSettings};
use taildesk_transport::{Identity, server_endpoint};
use taildesk_tsapi::LocalApi;
use taildesk_ui::{JanelaFaixa, JanelaPedido};
use tokio::sync::{mpsc, oneshot};

use crate::midia_host::{Fonte, MidiaApp};
use crate::{avisar, config, no_app};

/// Nome da identidade do host no cofre.
const SEGREDO_IDENTIDADE: &str = "identidade-host";

/// Identidade do host: a mesma em toda execução (os controladores fixam o fingerprint). Sem
/// cofre, uma nova a cada execução, e os controladores verão "certificado mudou".
fn identidade() -> Identity {
    use taildesk_platform::segredos;
    match segredos::ler(SEGREDO_IDENTIDADE) {
        Ok(Some(b)) => {
            if let Some(id) = Identity::from_secret(&b) {
                return id;
            }
            eprintln!("taildesk: identidade no cofre ilegível; gerando outra");
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("taildesk: {e}; identidade só desta execução");
            return Identity::generate().expect("gerar certificado");
        }
    }
    let id = Identity::generate().expect("gerar certificado");
    if let Err(e) = segredos::gravar(SEGREDO_IDENTIDADE, &id.to_secret()) {
        eprintln!("taildesk: {e}; identidade só desta execução");
    }
    id
}

/// Sobe o host quando o Tailscale tiver IP e atende até o app fechar.
pub async fn rodar(settings: Arc<Mutex<HostSettings>>) {
    let id = tokio::task::spawn_blocking(identidade)
        .await
        .expect("identidade");
    let overlay = Arc::new(SystemOverlay::new(LocalApi::default()));
    let endpoint = loop {
        if let Ok(ip) = overlay.local_addr().await {
            match server_endpoint(SocketAddr::new(ip, PORTA), &id) {
                Ok(e) => break e,
                Err(e) => eprintln!("taildesk: não foi possível abrir a porta {PORTA}: {e}"),
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    };
    let inbox = taildesk_platform::pasta_downloads().map(|d| d.join("TailDesk"));
    let config = Arc::new(ConfigHost {
        settings,
        inbox,
        raizes: Vec::new(),
        auditoria: None,
    });
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(servir(
        endpoint,
        overlay,
        Arc::new(MaquinaApp::default()),
        config,
        tx,
    ));
    while let Some(ev) = rx.recv().await {
        no_app(move |app| app.host.evento(ev, &app.settings));
    }
}

// ---------------------------------------------------------------------------------------------
// Mouse, teclado e vídeo

#[derive(Debug, Default)]
struct MaquinaApp {
    /// Uma sessão do portal (RemoteDesktop + ScreenCast) para o app todo: input e captura saem
    /// dela, e o diálogo do compositor aparece uma vez só.
    #[cfg(target_os = "linux")]
    portal: tokio::sync::Mutex<Option<Arc<taildesk_platform::wayland::SessaoPortal>>>,
}

#[cfg(target_os = "linux")]
impl MaquinaApp {
    /// Abre (uma vez) a sessão do portal: o compositor mostra o diálogo dele.
    async fn portal(&self) -> Option<Arc<taildesk_platform::wayland::SessaoPortal>> {
        use taildesk_platform::wayland::SessaoPortal;
        let mut portal = self.portal.lock().await;
        if portal.is_none() {
            match SessaoPortal::abrir(true).await {
                Ok(s) => *portal = Some(Arc::new(s)),
                Err(e) => {
                    eprintln!("taildesk: portal de controle remoto recusado ou indisponível: {e}");
                    avisar(format!("Sem acesso à tela e ao controle: {e}"));
                    return None;
                }
            }
        }
        portal.clone()
    }
}

/// Monitores liberados no diálogo do portal, como o protocolo descreve.
#[cfg(target_os = "linux")]
fn monitores_do_portal(
    portal: &taildesk_platform::wayland::SessaoPortal,
) -> Vec<taildesk_proto::control::DisplayInfo> {
    portal
        .monitores()
        .iter()
        .map(|m| {
            let (w, h) = (m.tamanho_logico.0.max(1), m.tamanho_logico.1.max(1));
            taildesk_proto::control::DisplayInfo {
                id: m.display_id,
                name: String::new(),
                x: m.posicao.0,
                y: m.posicao.1,
                width: w,
                height: h,
                logical_width: w,
                logical_height: h,
                scale: 1.0,
                refresh_mhz: 0,
                primary: m.display_id == 0,
                rotation: 0,
                shared: true,
            }
        })
        .collect()
}

fn nome_do_computador() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
        })
        .unwrap_or_default()
}

#[cfg(windows)]
fn teclado_do_host() -> (Keymap, String) {
    use taildesk_platform::teclado::windows;
    // O layout que vale é o da janela em foco no host.
    // SAFETY: sem pré-condições.
    let thread = unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };
        GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut())
    };
    let k = windows::ler_keymap_da_thread(thread);
    let id = k.name.clone();
    (k, id)
}

#[cfg(target_os = "linux")]
fn teclado_do_host() -> (Keymap, String) {
    use taildesk_platform::teclado::xkb;
    let (layout, variante) =
        xkb::layout_da_sessao().unwrap_or_else(|| ("us".into(), String::new()));
    match xkb::ler_keymap_nomes(&layout, &variante, &xkb::locale_da_sessao()) {
        Ok(k) => {
            let id = k.name.clone();
            (k, id)
        }
        Err(e) => {
            eprintln!("taildesk: layout {layout}({variante}) ilegível: {e}");
            (Keymap::default(), String::new())
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn teclado_do_host() -> (Keymap, String) {
    (Keymap::default(), String::new())
}

#[cfg(windows)]
fn monitores() -> Vec<taildesk_proto::control::DisplayInfo> {
    use taildesk_platform::captura_dxgi::listar_monitores;
    listar_monitores()
        .unwrap_or_default()
        .into_iter()
        .map(|m| taildesk_proto::control::DisplayInfo {
            id: m.display_id,
            name: String::new(),
            x: m.area.x,
            y: m.area.y,
            width: m.area.largura.max(1) as u32,
            height: m.area.altura.max(1) as u32,
            logical_width: m.area.largura.max(1) as u32,
            logical_height: m.area.altura.max(1) as u32,
            scale: 1.0,
            refresh_mhz: 0,
            primary: m.principal,
            rotation: 0,
            shared: true,
        })
        .collect()
}

/// No Linux os monitores só se sabem depois do diálogo do portal; a mídia manda
/// `DisplaysChanged` quando abrir.
#[cfg(not(windows))]
fn monitores() -> Vec<taildesk_proto::control::DisplayInfo> {
    Vec::new()
}

impl Maquina for MaquinaApp {
    type Entrada = EntradaApp;
    type Midia = MidiaApp;

    fn descricao(&self) -> Descricao {
        let (keymap, os_layout_id) = teclado_do_host();
        Descricao {
            nome: nome_do_computador(),
            plataforma: Platform {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                device_model: String::new(),
            },
            monitores: monitores(),
            teclado: KeyboardInfo {
                layout_tag: taildesk_keyboard::layout_tag(&os_layout_id),
                os_layout_id,
                extra_hid: Vec::new(),
            },
            keymap,
        }
    }

    #[cfg(windows)]
    async fn entrada(&self) -> Option<EntradaApp> {
        let monitores = taildesk_platform::captura_dxgi::listar_monitores()
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.display_id, m.area))
            .collect();
        Some(EntradaApp {
            teclado: Default::default(),
            mouse: Default::default(),
            presos: Default::default(),
            monitores,
            ultimo: None,
        })
    }

    #[cfg(target_os = "linux")]
    async fn entrada(&self) -> Option<EntradaApp> {
        Some(EntradaApp {
            portal: self.portal().await?,
            presos: Default::default(),
        })
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    async fn entrada(&self) -> Option<EntradaApp> {
        None
    }

    #[cfg(windows)]
    async fn midia(&self, canal: CanalMidia) -> MidiaApp {
        let monitores = monitores();
        // Sem monitor (sessão sem console, DXGI indisponível): nada a capturar.
        let fonte = if monitores.is_empty() {
            Fonte::Nenhuma
        } else {
            Fonte::Dxgi
        };
        MidiaApp::new(canal, fonte, monitores)
    }

    /// Os monitores só se sabem depois do diálogo do portal: saem num `DisplaysChanged`.
    #[cfg(target_os = "linux")]
    async fn midia(&self, canal: CanalMidia) -> MidiaApp {
        let Some(portal) = self.portal().await else {
            return MidiaApp::new(canal, Fonte::Nenhuma, Vec::new());
        };
        let monitores = monitores_do_portal(&portal);
        canal
            .enviar(Msg::DisplaysChanged(
                taildesk_proto::control::DisplaysChanged {
                    displays: monitores.clone(),
                },
            ))
            .await;
        MidiaApp::new(canal, Fonte::Portal(portal), monitores)
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    async fn midia(&self, canal: CanalMidia) -> MidiaApp {
        MidiaApp::new(canal, Fonte::Nenhuma, Vec::new())
    }
}

#[cfg(windows)]
pub struct EntradaApp {
    teclado: taildesk_platform::teclado::sendinput::InjetorWindows,
    mouse: taildesk_platform::mouse::sendinput::InjetorMouseWindows,
    presos: taildesk_platform::mouse::BotoesPresos,
    monitores: Vec<(u32, taildesk_platform::mouse::sendinput::Retangulo)>,
    /// Monitor do último movimento absoluto (botões e rolagem não dizem o monitor).
    ultimo: Option<taildesk_platform::mouse::sendinput::Retangulo>,
}

#[cfg(windows)]
impl EntradaApp {
    fn injetar_mouse(&mut self, ev: &taildesk_platform::mouse::EventoMouse) {
        use taildesk_platform::mouse::EventoMouse;
        if let EventoMouse::Absoluto { display_id, .. } = *ev {
            self.ultimo = self
                .monitores
                .iter()
                .find(|(id, _)| *id == display_id)
                .map(|(_, r)| *r);
        }
        let Some(r) = self
            .ultimo
            .or_else(|| self.monitores.first().map(|(_, r)| *r))
        else {
            return;
        };
        if let Err(e) = self.mouse.injetar(ev, r) {
            eprintln!("taildesk: mouse: {e}");
        }
    }
}

#[cfg(windows)]
impl Entrada for EntradaApp {
    fn travas(&self) -> Locks {
        taildesk_platform::teclado::sendinput::travas()
    }

    async fn teclas(&mut self, eventos: &[Inject]) {
        use taildesk_platform::teclado::InjetorTeclado;
        if let Err(e) = self.teclado.injetar(eventos) {
            eprintln!("taildesk: teclado: {e}");
        }
    }

    async fn mouse(&mut self, msg: &Msg) {
        let Some(ev) = taildesk_platform::mouse::EventoMouse::de_msg(msg) else {
            return;
        };
        if self.presos.registrar(&ev) {
            self.injetar_mouse(&ev);
        }
    }

    async fn soltar_mouse(&mut self) {
        for ev in self.presos.soltar_todos() {
            self.injetar_mouse(&ev);
        }
    }
}

#[cfg(target_os = "linux")]
pub struct EntradaApp {
    portal: Arc<taildesk_platform::wayland::SessaoPortal>,
    presos: taildesk_platform::mouse::BotoesPresos,
}

#[cfg(target_os = "linux")]
impl Entrada for EntradaApp {
    /// O portal não diz o estado dos locks; o modo Físico não sincroniza Caps/Num.
    fn travas(&self) -> Locks {
        Locks::default()
    }

    async fn teclas(&mut self, eventos: &[Inject]) {
        let injetor = taildesk_platform::teclado::portal::InjetorPortal::new(&self.portal);
        if let Err(e) = injetor.injetar(eventos).await {
            eprintln!("taildesk: teclado: {e}");
        }
    }

    async fn mouse(&mut self, msg: &Msg) {
        use taildesk_platform::mouse::{EventoMouse, portal};
        let Some(ev) = EventoMouse::de_msg(msg) else {
            return;
        };
        if self.presos.registrar(&ev)
            && let Err(e) = portal::injetar(&self.portal, &ev).await
        {
            eprintln!("taildesk: mouse: {e}");
        }
    }

    async fn soltar_mouse(&mut self) {
        for ev in self.presos.soltar_todos() {
            let _ = taildesk_platform::mouse::portal::injetar(&self.portal, &ev).await;
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub struct EntradaApp;

#[cfg(not(any(windows, target_os = "linux")))]
impl Entrada for EntradaApp {
    fn travas(&self) -> Locks {
        Locks::default()
    }
    async fn teclas(&mut self, _: &[Inject]) {}
    async fn mouse(&mut self, _: &Msg) {}
    async fn soltar_mouse(&mut self) {}
}

// ---------------------------------------------------------------------------------------------
// Interface (thread do Slint)

struct Pedido {
    janela: JanelaPedido,
    resposta: Option<oneshot::Sender<ConsentAnswer>>,
    _relogio: Timer,
}

struct Faixa {
    janela: JanelaFaixa,
}

/// Janelas do host abertas agora, por sessão.
#[derive(Default)]
pub struct Ui {
    pedidos: std::cell::RefCell<HashMap<IdSessao, Pedido>>,
    faixas: std::cell::RefCell<HashMap<IdSessao, Faixa>>,
}

impl std::fmt::Debug for Ui {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ui").finish_non_exhaustive()
    }
}

fn gravar_settings(settings: &Arc<Mutex<HostSettings>>) {
    let host = settings.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut c = config::ler();
    c.host = host;
    if let Err(e) = config::gravar(&c) {
        eprintln!("taildesk: não foi possível gravar a configuração: {e}");
    }
}

/// Responde o pedido `sessao` (uma vez só) e fecha o diálogo.
fn responder(sessao: IdSessao, resposta: ConsentAnswer) {
    no_app(move |app| {
        let pedido = app.host.pedidos.borrow_mut().remove(&sessao);
        if let Some(mut p) = pedido {
            if let Some(r) = p.resposta.take() {
                let _ = r.send(resposta);
            }
            let _ = p.janela.hide();
        }
    });
}

impl Ui {
    /// Algum pedido de acesso ou sessão ao vivo neste host.
    pub fn ativa(&self) -> bool {
        !self.pedidos.borrow().is_empty() || !self.faixas.borrow().is_empty()
    }

    pub fn evento(&self, ev: EventoHost, settings: &Arc<Mutex<HostSettings>>) {
        match ev {
            EventoHost::Pedido {
                sessao,
                prompt,
                resposta,
            } => self.pedir(sessao, prompt, resposta),
            EventoHost::AoVivo {
                sessao,
                peer,
                permissoes,
                comandos,
            } => {
                self.fechar_pedido(sessao);
                // "Lembrar" pode ter mudado a lista.
                gravar_settings(settings);
                self.faixa(sessao, &peer.hostname, permissoes, comandos);
            }
            EventoHost::Fim { sessao, .. } => {
                self.fechar_pedido(sessao);
                if let Some(f) = self.faixas.borrow_mut().remove(&sessao) {
                    let _ = f.janela.hide();
                }
            }
            EventoHost::Arquivos { evento, .. } => {
                if let taildesk_files::service::Event::Finished { path, .. } = evento {
                    avisar(format!("Arquivo recebido: {}", path.display()));
                }
            }
            // O vídeo vai direto para a `MidiaApp`; aqui só chegaria o que ninguém trata ainda.
            EventoHost::Mensagem { .. } => {}
        }
    }

    fn fechar_pedido(&self, sessao: IdSessao) {
        if let Some(p) = self.pedidos.borrow_mut().remove(&sessao) {
            let _ = p.janela.hide();
        }
    }

    fn pedir(
        &self,
        sessao: IdSessao,
        prompt: ConsentPrompt,
        resposta: oneshot::Sender<ConsentAnswer>,
    ) {
        let janela = match JanelaPedido::new() {
            Ok(j) => j,
            Err(e) => {
                // Sem diálogo não há consentimento: recusa.
                eprintln!("taildesk: diálogo de pedido: {e}");
                let _ = resposta.send(ConsentAnswer::Refuse);
                return;
            }
        };
        let p = &prompt.peer;
        janela.set_peer(p.hostname.as_str().into());
        let mut detalhe = Vec::new();
        if let Some(u) = &p.user_login {
            detalhe.push(format!("Usuário: {u}"));
        }
        if !p.tags.is_empty() {
            detalhe.push(p.tags.join(", "));
        }
        janela.set_detalhe(detalhe.join(" · ").into());
        janela.set_oferece_controle(prompt.offer.control);
        janela.set_oferece_arquivos(prompt.offer.files);
        let offer = prompt.offer;
        let fraca = janela.as_weak();
        janela.on_permitir(move || {
            let Some(j) = fraca.upgrade() else { return };
            let permissions = Permissions {
                view: true,
                control: offer.control && j.get_controlar(),
                files: offer.files && j.get_arquivos(),
                clipboard: offer.clipboard,
                audio: offer.audio,
            };
            responder(
                sessao,
                ConsentAnswer::Allow {
                    permissions,
                    remember: j.get_lembrar(),
                },
            );
        });
        janela.on_recusar(move || responder(sessao, ConsentAnswer::Refuse));
        janela.window().on_close_requested(move || {
            responder(sessao, ConsentAnswer::Refuse);
            CloseRequestResponse::HideWindow
        });
        let relogio = Timer::default();
        let (fraca, prazo) = (janela.as_weak(), prompt.deadline);
        relogio.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            if let Some(j) = fraca.upgrade() {
                let falta = prazo.saturating_duration_since(Instant::now());
                j.set_restante(falta.as_secs_f32().ceil() as i32);
            }
        });
        if let Err(e) = janela.show() {
            eprintln!("taildesk: diálogo de pedido: {e}");
        }
        self.pedidos.borrow_mut().insert(
            sessao,
            Pedido {
                janela,
                resposta: Some(resposta),
                _relogio: relogio,
            },
        );
    }

    fn faixa(
        &self,
        sessao: IdSessao,
        quem: &str,
        p: Permissions,
        comandos: mpsc::Sender<ComandoHost>,
    ) {
        let Ok(janela) = JanelaFaixa::new() else {
            return;
        };
        janela.set_texto(
            if p.control {
                format!("{quem} controla este computador")
            } else {
                format!("{quem} vê a tela deste computador")
            }
            .into(),
        );
        janela.set_pode_pausar(p.control);
        let c = comandos.clone();
        janela.on_pausar(move |v| {
            let _ = c.try_send(ComandoHost::PausarControle(v));
        });
        let c = comandos.clone();
        janela.on_encerrar(move || {
            let _ = c.try_send(ComandoHost::Encerrar);
        });
        // Fechar a faixa encerra: nunca deixar alguém controlando sem aviso na tela.
        janela.window().on_close_requested(move || {
            let _ = comandos.try_send(ComandoHost::Encerrar);
            CloseRequestResponse::KeepWindowShown
        });
        let _ = janela.show();
        self.faixas.borrow_mut().insert(sessao, Faixa { janela });
    }
}
