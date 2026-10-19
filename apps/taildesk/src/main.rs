//! App desktop do TailDesk: janela principal com os dispositivos da tailnet e ícone na bandeja
//! do sistema. Fechar a janela deixa o app na bandeja; "Sair" no menu do ícone encerra.
//!
//! Uso: `taildesk [--oculto]`. `--oculto` abre só na bandeja (para iniciar com o sistema). No
//! Windows também `--servico`, `--instalar-servico` e `--remover-servico` (serviço do Windows,
//! `taildesk_platform::servico_windows`).
//! Abrir o TailDesk de novo só mostra a janela da instância que já roda.
//!
//! Integração nativa com o desktop: no Wayland a janela é um xdg_toplevel com app_id
//! `io.github.yurisanp.TailDesk` (o mesmo nome do `.desktop`, de onde o compositor tira ícone e agrupamento), sem XWayland, e
//! ganha o foco pelo token de xdg-activation que o lançador entrega. Como o Wayland não esconde
//! janelas, fechar destrói a janela e abrir cria outra, com o token novo.

// Sem console no Windows: é um app gráfico.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod area;
mod atualizacao;
mod config;
mod controlador;
mod historico;
mod host;
mod midia_host;
mod modelo;
mod preferencias;
mod som;
mod video;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, VecModel};
use taildesk_platform::instancia::{self, Instancia};
use taildesk_tsapi::LocalApi;
use taildesk_ui::{Bandeja, Dispositivo, JanelaPrincipal};
use tokio::sync::Notify;

use modelo::Visao;

/// De quanto em quanto tempo o app relê o estado do Tailscale.
const INTERVALO: Duration = Duration::from_secs(3);

/// Nome do `.desktop` (`packaging/linux/io.github.yurisanp.TailDesk.desktop`), do ícone e do
/// MetaInfo: app_id no Wayland, WM_CLASS no X11 e id do Flatpak, que o Flathub exige iguais.
const APP_ID: &str = "io.github.yurisanp.TailDesk";

/// Estado que vive na thread da interface. A janela só existe enquanto está aberta.
struct App {
    janela: RefCell<Option<JanelaPrincipal>>,
    todos: RefCell<Vec<Dispositivo>>,
    lista: Rc<VecModel<Dispositivo>>,
    visao: RefCell<Visao>,
    aceitando: Cell<bool>,
    /// Ícone na bandeja; `None` quando o sistema não tem bandeja.
    bandeja: Option<Bandeja>,
    agora: Arc<Notify>,
    /// "Este computador": lido do arquivo, mudado pelo diálogo e por "Aceitar conexões".
    settings: Arc<Mutex<taildesk_session::HostSettings>>,
    /// Pedido de acesso e faixa de sessão ativa.
    host: host::Ui,
    /// Janelas de sessão abertas por este computador.
    controlador: controlador::Controlador,
    rt: tokio::runtime::Handle,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
    /// Token de ativação para a próxima janela criada (consumido pelo gancho do winit).
    static TOKEN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Roda `f` com o app agora; só na thread da interface. `None` se o app já fechou.
fn com_app<R>(f: impl FnOnce(&App) -> R) -> Option<R> {
    APP.with(|a| a.borrow().clone()).map(|app| f(&app))
}

/// Roda `f` com o app, na thread da interface (de qualquer thread).
fn no_app(f: impl FnOnce(&App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = APP.with(|a| a.borrow().clone()) {
            f(&app);
        }
    });
}

impl App {
    /// Mostra a janela (criando-a se estiver fechada) e pede o foco.
    fn mostrar(&self, token: Option<String>) {
        let criada = self.janela.borrow().is_none();
        if criada {
            TOKEN.with(|t| *t.borrow_mut() = token);
            match self.criar_janela() {
                Ok(j) => *self.janela.borrow_mut() = Some(j),
                Err(e) => {
                    eprintln!("taildesk: não foi possível criar a janela: {e}");
                    return;
                }
            }
        }
        let Some(j) = self.janela.borrow().as_ref().map(|j| j.clone_strong()) else {
            return;
        };
        if let Err(e) = j.show() {
            eprintln!("taildesk: não foi possível mostrar a janela: {e}");
        }
        j.window().set_minimized(false);
        // No Wayland o foco vem só do token de ativação (o compositor decide). No Windows e no
        // X11, a janela pede para vir à frente.
        use slint::winit_030::WinitWindowAccessor;
        j.window().with_winit_window(|w| {
            if !eh_wayland(w) {
                w.focus_window();
            }
        });
    }

    fn criar_janela(&self) -> Result<JanelaPrincipal, slint::PlatformError> {
        let j = JanelaPrincipal::new()?;
        j.set_linux(cfg!(target_os = "linux"));
        j.set_versao_app(env!("CARGO_PKG_VERSION").into());
        j.set_bandeja(self.bandeja.is_some());
        j.set_aceitando(self.aceitando.get());
        j.set_dispositivos(ModelRc::from(self.lista.clone()));
        aplicar_na_janela(&j, &self.visao.borrow());
        atualizacao::ligar(&j);
        preferencias::ligar(&j);

        j.on_busca_mudou(|_| no_app(App::filtrar));
        j.on_aceitando_mudou(|v| no_app(move |app| app.definir_aceitando(v)));
        j.on_abrir_link(|url| {
            if let Err(e) = taildesk_platform::abrir_link(&url) {
                avisar(format!("Não foi possível abrir o navegador: {e}"));
            }
        });
        j.on_conectar(|d| {
            com_app(|app| {
                app.controlador
                    .conectar(&app.rt, d.nome.to_string(), d.ip.to_string(), None)
            });
        });
        let agora = self.agora.clone();
        j.on_verificar(move || agora.notify_one());
        let com_bandeja = self.bandeja.is_some();
        j.window().on_close_requested(move || {
            if com_bandeja {
                // Solta a janela depois que o Slint terminar de escondê-la.
                no_app(|app| drop(app.janela.borrow_mut().take()));
            } else {
                let _ = slint::quit_event_loop();
            }
            CloseRequestResponse::HideWindow
        });
        Ok(j)
    }

    /// Reaplica a busca na lista, mantendo o dispositivo selecionado quando ele continua lá.
    fn filtrar(&self) {
        let janela = self.janela.borrow();
        let (busca, sel) = janela
            .as_ref()
            .map(|j| (j.get_busca(), j.get_selecionado()))
            .unwrap_or_default();
        let anterior = self.lista.row_data(sel.max(0) as usize).map(|d| d.nome);
        let filtrados = modelo::filtrar(&self.todos.borrow(), &busca);
        let selecionado = anterior
            .and_then(|n| filtrados.iter().position(|d| d.nome == n))
            .unwrap_or(0);
        self.lista.set_vec(filtrados);
        if let Some(j) = janela.as_ref() {
            j.set_selecionado(selecionado as i32);
        }
    }

    fn aplicar(&self, v: Visao) {
        if let Some(j) = self.janela.borrow().as_ref() {
            aplicar_na_janela(j, &v);
        }
        if let Some(b) = &self.bandeja {
            b.set_estado(v.linha_bandeja().into());
        }
        *self.todos.borrow_mut() = v.dispositivos.clone();
        *self.visao.borrow_mut() = v;
        self.filtrar();
    }

    /// Há sessão aberta, como host ou como controlador (o atualizador não troca o app no meio).
    fn sessao_ativa(&self) -> bool {
        self.host.ativa() || self.controlador.ativa()
    }

    /// Redesenha as Configurações (lista de confiáveis mudou pelo pedido de acesso, por exemplo).
    fn atualizar_configuracoes(&self) {
        if let Some(j) = self.janela.borrow().as_ref() {
            preferencias::mostrar(j);
        }
    }

    /// Mantém a janela e a bandeja com o mesmo valor de "Aceitar conexões".
    fn definir_aceitando(&self, aceitando: bool) {
        self.aceitando.set(aceitando);
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .accept_connections = aceitando;
        if let Some(j) = self.janela.borrow().as_ref() {
            j.set_aceitando(aceitando);
        }
        if let Some(b) = &self.bandeja {
            b.set_aceitando(aceitando);
        }
    }
}

fn aplicar_na_janela(j: &JanelaPrincipal, v: &Visao) {
    j.set_tailscale(v.estado);
    j.set_tailscale_resumo(v.resumo.as_str().into());
    j.set_tailscale_detalhe(v.detalhe.as_str().into());
    j.set_versao_tailscale(v.versao.as_str().into());
    j.set_versao_antiga(v.versao_antiga);
    j.set_este_pc(v.este_pc.as_str().into());
    j.set_total(v.dispositivos.len() as i32);
}

/// Mensagem curta no rodapé da janela, se ela estiver aberta.
fn avisar(texto: String) {
    no_app(move |app| {
        if let Some(j) = app.janela.borrow().as_ref() {
            j.set_toast(texto.into());
        }
    });
}

/// Aplica o token de ativação pendente (Linux: xdg-activation no Wayland, startup-notification
/// no X11) aos atributos da próxima janela.
#[cfg(target_os = "linux")]
fn com_token(
    atributos: slint::winit_030::winit::window::WindowAttributes,
) -> slint::winit_030::winit::window::WindowAttributes {
    use slint::winit_030::winit::platform::startup_notify::WindowAttributesExtStartupNotify;
    use slint::winit_030::winit::window::ActivationToken;
    match TOKEN.with(|t| t.borrow_mut().take()) {
        Some(t) => atributos.with_activation_token(ActivationToken::from_raw(t)),
        None => atributos,
    }
}

/// Fora do Linux não há token: o Windows usa `AllowSetForegroundWindow` (ver `instancia`).
#[cfg(not(target_os = "linux"))]
fn com_token(
    atributos: slint::winit_030::winit::window::WindowAttributes,
) -> slint::winit_030::winit::window::WindowAttributes {
    TOKEN.with(|t| t.borrow_mut().take());
    atributos
}

fn eh_wayland(w: &slint::winit_030::winit::window::Window) -> bool {
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    w.window_handle()
        .is_ok_and(|h| matches!(h.as_raw(), RawWindowHandle::Wayland(_)))
}

/// Lê o Tailscale do sistema a cada [`INTERVALO`] (ou quando `agora` é avisado) e manda o
/// resultado para a interface.
async fn acompanhar_tailscale(agora: Arc<Notify>) {
    let api = LocalApi::default();
    loop {
        let visao = Visao::de(&api.status().await);
        no_app(move |app| app.aplicar(visao));
        tokio::select! {
            _ = tokio::time::sleep(INTERVALO) => {}
            _ = agora.notified() => {}
        }
    }
}

fn criar_bandeja(aceitando: bool) -> Option<Bandeja> {
    if !taildesk_platform::bandeja_disponivel() {
        eprintln!(
            "taildesk: o sistema não mostra ícones na bandeja; fechar a janela encerra o app"
        );
        return None;
    }
    let bandeja = match Bandeja::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("taildesk: bandeja indisponível ({e}); fechar a janela encerra o app");
            return None;
        }
    };
    bandeja.set_estado(Visao::verificando().linha_bandeja().into());
    bandeja.set_aceitando(aceitando);
    bandeja.on_abrir(|| no_app(|app| app.mostrar(None)));
    bandeja.on_aceitando_mudou(|v| no_app(move |app| app.definir_aceitando(v)));
    bandeja.on_sair(|| {
        let _ = slint::quit_event_loop();
    });
    Some(bandeja)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: primeira coisa do main, antes de qualquer outra thread existir.
    let token = unsafe { taildesk_platform::tomar_token_de_ativacao() };

    let mut oculto = false;
    let mut args = std::env::args().skip(1);
    // `--agente` (Windows) lê o argumento seguinte dentro do laço.
    #[cfg_attr(not(windows), allow(clippy::while_let_on_iterator))]
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--oculto" | "--hidden" => oculto = true,
            // Agente da tela de entrada (lançado pelo serviço como SYSTEM quando ninguém está
            // logado): host sem interface com a identidade do último usuário.
            #[cfg(windows)]
            "--agente" => {
                let perfil = args.next().ok_or("uso: taildesk --agente <perfil>")?;
                return Ok(host::rodar_agente(std::path::PathBuf::from(perfil))?);
            }
            // Serviço do Windows ("Iniciar com o sistema"): o Gerenciador de Serviços roda
            // `--servico`; o app se chama como administrador para instalar e remover.
            #[cfg(windows)]
            "--servico" => return Ok(taildesk_platform::servico_windows::rodar()?),
            #[cfg(windows)]
            "--instalar-servico" => {
                let exe = taildesk_platform::inicio::executavel()?;
                return Ok(taildesk_platform::servico_windows::instalar(&exe)?);
            }
            #[cfg(windows)]
            "--remover-servico" => return Ok(taildesk_platform::servico_windows::remover()?),
            "--version" | "-V" => {
                println!("taildesk {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => {
                eprintln!("uso: taildesk [--oculto]");
                std::process::exit(2);
            }
        }
    }

    // Outra instância já aberta: ela mostra a janela (com o nosso token) e esta sai.
    let _guarda = match instancia::garantir("app", token.as_deref(), |t| {
        no_app(move |app| app.mostrar(t))
    }) {
        Ok(Instancia::Primeira(g)) => Some(g),
        Ok(Instancia::JaAberta) => return Ok(()),
        Err(e) => {
            eprintln!("taildesk: instância única indisponível ({e}); seguindo assim mesmo");
            None
        }
    };

    // "Iniciar com o sistema" apontando para outro executável (AppImage movido): corrige.
    std::thread::spawn(|| {
        use taildesk_platform::inicio;
        if inicio::desatualizado()
            && inicio::ligado()
            && let Err(e) = inicio::ligar()
        {
            eprintln!("taildesk: não foi possível atualizar o início com o sistema: {e}");
        }
    });

    // Coordenadas do mouse e da captura em pixels reais em cada monitor.
    #[cfg(windows)]
    taildesk_platform::captura_dxgi::dpi_por_monitor();

    // Backend winit nativo (Wayland quando houver, X11 ou Win32), com o token de ativação
    // aplicado na criação de cada janela. Com GPU, o renderer `wgpu` com o nosso device (o
    // vídeo vira textura sem passar pela CPU); sem, o renderer padrão.
    let com_gpu = video::preparar_gpu().and_then(|cfg| {
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .require_wgpu_30(cfg)
            .with_winit_window_attributes_hook(com_token)
            .select()
            .map_err(|e| eprintln!("taildesk: renderer wgpu indisponível ({e}); vídeo pela CPU"))
            .ok()
    });
    if com_gpu.is_none() {
        video::sem_gpu();
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .with_winit_window_attributes_hook(com_token)
            .select()?;
    }
    slint::set_xdg_app_id(APP_ID)?;

    let aceitando = true;
    let bandeja = criar_bandeja(aceitando);
    let com_bandeja = bandeja.is_some();

    // Leitura do Tailscale numa thread própria, para não travar a interface.
    let agora = Arc::new(Notify::new());
    let conf = config::ler();
    let mut settings = conf.host;
    settings.accept_connections = aceitando;
    let settings = Arc::new(Mutex::new(settings));
    // Rede (Tailscale, sessões) fora da thread da interface.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("taildesk-rede")
        .enable_all()
        .build()?;
    let app = Rc::new(App {
        janela: RefCell::new(None),
        todos: RefCell::new(Vec::new()),
        lista: Rc::new(VecModel::default()),
        visao: RefCell::new(Visao::verificando()),
        aceitando: Cell::new(aceitando),
        bandeja,
        agora: agora.clone(),
        settings: settings.clone(),
        host: host::Ui::default(),
        controlador: controlador::Controlador::default(),
        rt: rt.handle().clone(),
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));
    rt.spawn(acompanhar_tailscale(agora));
    rt.spawn(host::rodar(settings));
    atualizacao::iniciar(conf.atualizacao);

    if !(oculto && com_bandeja) {
        app.mostrar(token);
    }
    drop(app);
    slint::run_event_loop_until_quit()?;

    // Fecha a janela e tira o ícone da bandeja antes de sair.
    APP.with(|a| a.borrow_mut().take());
    rt.shutdown_timeout(Duration::from_secs(1));
    // O exec não roda destrutores: tira o socket da instância única antes (no Windows a guarda
    // não tem nada a soltar).
    #[cfg(unix)]
    drop(_guarda);
    atualizacao::reiniciar_se_pedido();
    Ok(())
}
