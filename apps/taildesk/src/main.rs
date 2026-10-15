//! App desktop do TailDesk: janela principal com os dispositivos da tailnet e ícone na bandeja
//! do sistema. Fechar a janela deixa o app na bandeja; "Sair" no menu do ícone encerra.
//!
//! Uso: `taildesk [--oculto]`. `--oculto` abre só na bandeja (para iniciar com o sistema).
//! Abrir o TailDesk de novo só mostra a janela da instância que já roda.

// Sem console no Windows: é um app gráfico.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod modelo;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, VecModel};
use taildesk_platform::instancia::{self, Instancia};
use taildesk_tsapi::LocalApi;
use taildesk_ui::{Bandeja, Dispositivo, JanelaPrincipal};
use tokio::sync::Notify;

use modelo::Visao;

/// De quanto em quanto tempo o app relê o estado do Tailscale.
const INTERVALO: Duration = Duration::from_secs(3);

/// Estado que vive na thread da interface.
struct App {
    janela: JanelaPrincipal,
    todos: RefCell<Vec<Dispositivo>>,
    lista: Rc<VecModel<Dispositivo>>,
    /// Ícone na bandeja; `None` quando o sistema não tem bandeja.
    bandeja: Option<Bandeja>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
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
    fn mostrar(&self) {
        let w = self.janela.window();
        if let Err(e) = self.janela.show() {
            eprintln!("taildesk: não foi possível mostrar a janela: {e}");
        }
        w.set_minimized(false);
    }

    /// Reaplica a busca na lista, mantendo o dispositivo selecionado quando ele continua lá.
    fn filtrar(&self) {
        let janela = &self.janela;
        let anterior = self
            .lista
            .row_data(janela.get_selecionado().max(0) as usize)
            .map(|d| d.nome.clone());
        let filtrados = modelo::filtrar(&self.todos.borrow(), &janela.get_busca());
        let selecionado = anterior
            .and_then(|n| filtrados.iter().position(|d| d.nome == n))
            .unwrap_or(0);
        self.lista.set_vec(filtrados);
        janela.set_selecionado(selecionado as i32);
        janela.set_total(self.todos.borrow().len() as i32);
    }

    fn aplicar(&self, v: Visao) {
        let j = &self.janela;
        j.set_tailscale(v.estado);
        j.set_tailscale_resumo(v.resumo.as_str().into());
        j.set_tailscale_detalhe(v.detalhe.as_str().into());
        j.set_versao_tailscale(v.versao.as_str().into());
        j.set_versao_antiga(v.versao_antiga);
        j.set_este_pc(v.este_pc.as_str().into());
        if let Some(b) = &self.bandeja {
            b.set_estado(v.linha_bandeja().into());
        }
        *self.todos.borrow_mut() = v.dispositivos;
        self.filtrar();
    }

    /// Mantém a janela e a bandeja com o mesmo valor de "Aceitar conexões".
    fn definir_aceitando(&self, aceitando: bool) {
        self.janela.set_aceitando(aceitando);
        if let Some(b) = &self.bandeja {
            b.set_aceitando(aceitando);
        }
    }
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
    bandeja.set_estado(modelo::Visao::verificando().linha_bandeja().into());
    bandeja.set_aceitando(aceitando);
    bandeja.on_abrir(|| no_app(App::mostrar));
    bandeja.on_aceitando_mudou(|v| no_app(move |app| app.definir_aceitando(v)));
    bandeja.on_sair(|| {
        let _ = slint::quit_event_loop();
    });
    Some(bandeja)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut oculto = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--oculto" | "--hidden" => oculto = true,
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

    // Outra instância já aberta: ela mostra a janela e esta sai.
    let _guarda = match instancia::garantir("app", || no_app(App::mostrar)) {
        Ok(Instancia::Primeira(g)) => Some(g),
        Ok(Instancia::JaAberta) => return Ok(()),
        Err(e) => {
            eprintln!("taildesk: instância única indisponível ({e}); seguindo assim mesmo");
            None
        }
    };

    let janela = JanelaPrincipal::new()?;
    janela.set_linux(cfg!(target_os = "linux"));
    janela.set_versao_app(env!("CARGO_PKG_VERSION").into());
    let lista = Rc::new(VecModel::<Dispositivo>::default());
    janela.set_dispositivos(ModelRc::from(lista.clone()));

    let bandeja = criar_bandeja(janela.get_aceitando());
    let com_bandeja = bandeja.is_some();
    janela.set_bandeja(com_bandeja);

    janela.on_busca_mudou(|_| no_app(App::filtrar));
    janela.on_aceitando_mudou(|v| no_app(move |app| app.definir_aceitando(v)));
    janela.on_abrir_link(|url| {
        if let Err(e) = taildesk_platform::abrir_link(&url) {
            no_app(move |app| {
                app.janela
                    .set_toast(format!("Não foi possível abrir o navegador: {e}").into());
            });
        }
    });
    janela.on_conectar(|d| {
        let aviso = format!(
            "A conexão com {} chega nas próximas etapas da fase 1 (captura e vídeo).",
            d.nome
        );
        no_app(move |app| app.janela.set_toast(aviso.into()));
    });
    janela.window().on_close_requested(move || {
        if !com_bandeja {
            let _ = slint::quit_event_loop();
        }
        CloseRequestResponse::HideWindow
    });

    APP.with(|a| {
        *a.borrow_mut() = Some(Rc::new(App {
            janela: janela.clone_strong(),
            todos: RefCell::new(Vec::new()),
            lista,
            bandeja,
        }))
    });

    // Leitura do Tailscale numa thread própria, para não travar a interface.
    let agora = Arc::new(Notify::new());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let leitura = agora.clone();
    std::thread::Builder::new()
        .name("taildesk-tailscale".into())
        .spawn(move || rt.block_on(acompanhar_tailscale(leitura)))?;
    janela.on_verificar(move || agora.notify_one());

    if !(oculto && com_bandeja) {
        janela.show()?;
    }
    slint::run_event_loop_until_quit()?;

    // Tira o ícone da bandeja antes de sair.
    APP.with(|a| a.borrow_mut().take());
    Ok(())
}
