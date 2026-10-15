//! Protótipo estático da interface do TailDesk: telas e fluxos de `docs/design/12-levantamento-ux.md`
//! com dados de exemplo, sem rede, captura ou input reais.
//!
//! `cargo run -p taildesk-prototipo` abre a lista de dispositivos; "Conectar" abre a janela da
//! sessão e "Simular pedido recebido" abre o pedido de acesso do lado host.
//! `--tela <nome>` abre uma tela isolada (útil para capturas) e `--tema claro|escuro` força o tema.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

mod ui {
    #![allow(missing_debug_implementations, clippy::all)]
    slint::include_modules!();
}

use ui::{
    Dispositivo, Estado, Etapa, FaixaHost, JanelaPedido, JanelaPrincipal, JanelaSessao, Monitor,
    Tema,
};

const TELAS: &str = "principal, transferencias, configuracoes, pedido, faixa, sessao, sessao-monitores, sessao-wayland, sessao-com-janela, sessao-destacada, sessao-arquivos, sessao-navegar, sessao-relay, sessao-esperando, sessao-reconectando";

fn dispositivo(
    nome: &str,
    so: &str,
    ip: &str,
    estado: Estado,
    favorito: bool,
    ultimo: &str,
    teclado: &str,
) -> Dispositivo {
    Dispositivo {
        nome: nome.into(),
        so: so.into(),
        ip: ip.into(),
        dns: format!("{nome}.tail4f2a.ts.net").into(),
        estado,
        favorito,
        ultimo_acesso: ultimo.into(),
        teclado: teclado.into(),
    }
}

fn dispositivos_exemplo() -> Vec<Dispositivo> {
    vec![
        dispositivo(
            "desktop-casa",
            "Windows 11",
            "100.101.12.4",
            Estado::Online,
            true,
            "hoje 09:15",
            "US-Intl",
        ),
        dispositivo(
            "servidor-homelab",
            "Ubuntu 24.04 (Wayland)",
            "100.92.7.31",
            Estado::Relay,
            false,
            "ontem",
            "ABNT2",
        ),
        dispositivo(
            "notebook-trabalho",
            "Windows 11",
            "100.77.140.2",
            Estado::SemApp,
            false,
            "nunca",
            "ABNT2",
        ),
        dispositivo(
            "pc-mae",
            "Windows 10",
            "100.64.33.9",
            Estado::Offline,
            false,
            "há 3 dias",
            "ABNT2",
        ),
        dispositivo(
            "celular-yuri",
            "Android",
            "100.120.5.18",
            Estado::NaoSuportado,
            false,
            "—",
            "—",
        ),
    ]
}

fn monitor(nome: &str, resolucao: &str, principal: bool, liberado: bool) -> Monitor {
    Monitor {
        nome: nome.into(),
        resolucao: resolucao.into(),
        principal,
        liberado,
        destacado: false,
    }
}

/// Monitores de exemplo por dispositivo: o host Wayland tem um monitor que a pessoa não liberou no portal.
fn monitores_do(nome: &str) -> Vec<Monitor> {
    match nome {
        "desktop-casa" => vec![
            monitor("DELL U2720Q", "2560×1440", true, true),
            monitor("LG 24MK600", "1920×1080", false, true),
        ],
        "servidor-homelab" => vec![
            monitor("eDP-1", "1920×1200", true, true),
            monitor("DP-2", "2560×1440", false, true),
            monitor("HDMI-1", "1920×1080", false, false),
        ],
        _ => vec![monitor("Monitor", "1920×1080", true, true)],
    }
}

fn em_abas(monitores: &VecModel<Monitor>) -> i32 {
    monitores
        .iter()
        .filter(|m| m.liberado && !m.destacado)
        .count() as i32
}

/// Marca o monitor como destacado (ou não) e atualiza a janela principal da sessão.
fn marcar_destacado(
    sessao: &JanelaSessao,
    monitores: &VecModel<Monitor>,
    indice: usize,
    destacado: bool,
) {
    let Some(mut m) = monitores.row_data(indice) else {
        return;
    };
    m.destacado = destacado;
    monitores.set_row_data(indice, m);
    sessao.set_monitores_em_abas(em_abas(monitores));
    if destacado && sessao.get_monitor() as usize == indice {
        if let Some(livre) = monitores.iter().position(|m| m.liberado && !m.destacado) {
            sessao.set_monitor(livre as i32);
        }
    }
}

/// Abre um monitor da sessão em janela própria; fechar a janela devolve a aba.
fn destacar_monitor(
    sessao: &JanelaSessao,
    monitores: &Rc<VecModel<Monitor>>,
    indice: i32,
    tema: TemaEscolhido,
    abertas: &Rc<RefCell<Abertas>>,
) {
    marcar_destacado(sessao, monitores, indice as usize, true);
    let janela = JanelaSessao::new().expect("janela do monitor");
    aplicar_tema(&janela, tema);
    janela.set_host(sessao.get_host());
    janela.set_teclado_host(sessao.get_teclado_host());
    janela.set_via_relay(sessao.get_via_relay());
    janela.set_monitores(ModelRc::from(monitores.clone()));
    janela.set_monitor_unico(indice);
    janela.set_simular(false);
    janela.set_etapa(Etapa::Conectada);

    let devolver = {
        let sessao = sessao.as_weak();
        let janela = janela.as_weak();
        let monitores = monitores.clone();
        move || {
            if let Some(s) = sessao.upgrade() {
                marcar_destacado(&s, &monitores, indice as usize, false);
            }
            if let Some(j) = janela.upgrade() {
                let _ = j.hide();
            }
        }
    };
    let devolver_ao_fechar = devolver.clone();
    janela.on_voltar_para_abas(devolver);
    janela.window().on_close_requested(move || {
        devolver_ao_fechar();
        slint::CloseRequestResponse::HideWindow
    });
    // Desconectar numa janela destacada encerra a sessão inteira.
    let sessao_fraca = sessao.as_weak();
    janela.on_desconectar(move || {
        if let Some(s) = sessao_fraca.upgrade() {
            s.invoke_desconectar();
        }
    });
    janela.show().expect("mostrar janela do monitor");
    abertas.borrow_mut().sessoes.push(janela);
}

/// Liga os monitores de exemplo à janela da sessão e o "abrir em janela".
fn ligar_monitores(
    sessao: &JanelaSessao,
    nome: &str,
    tema: TemaEscolhido,
    abertas: &Rc<RefCell<Abertas>>,
) {
    let monitores = Rc::new(VecModel::from(monitores_do(nome)));
    sessao.set_monitores(ModelRc::from(monitores.clone()));
    sessao.set_monitores_em_abas(em_abas(&monitores));
    let fraca = sessao.as_weak();
    let abertas = abertas.clone();
    sessao.on_destacar(move |i| {
        if let Some(s) = fraca.upgrade() {
            destacar_monitor(&s, &monitores, i, tema, &abertas);
        }
    });
}

#[derive(Clone, Copy, PartialEq)]
enum TemaEscolhido {
    Sistema,
    Claro,
    Escuro,
}

fn aplicar_tema<C: ComponentHandle>(janela: &C, tema: TemaEscolhido)
where
    for<'a> Tema<'a>: slint::Global<'a, C>,
{
    match tema {
        TemaEscolhido::Sistema => {}
        TemaEscolhido::Claro => janela.global::<Tema>().set_escuro(false),
        TemaEscolhido::Escuro => janela.global::<Tema>().set_escuro(true),
    }
}

/// Janelas abertas além da principal; ficam vivas enquanto estiverem aqui.
#[derive(Default)]
struct Abertas {
    sessoes: Vec<JanelaSessao>,
    pedido: Option<JanelaPedido>,
    faixa: Option<FaixaHost>,
}

fn abrir_sessao(
    d: &Dispositivo,
    tema: TemaEscolhido,
    abertas: &Rc<RefCell<Abertas>>,
) -> JanelaSessao {
    let sessao = JanelaSessao::new().expect("janela da sessão");
    aplicar_tema(&sessao, tema);
    sessao.set_host(d.nome.clone());
    sessao.set_so_host(d.so.clone());
    sessao.set_teclado_host(d.teclado.clone());
    sessao.set_via_relay(d.estado == Estado::Relay);
    sessao.set_host_wayland(d.so.contains("Wayland"));
    ligar_monitores(&sessao, &d.nome, tema, abertas);
    let fraca = sessao.as_weak();
    let abertas_desconectar = abertas.clone();
    sessao.on_desconectar(move || {
        if let Some(s) = fraca.upgrade() {
            let _ = s.hide();
            // Encerrar a sessão fecha também as janelas de monitores destacados.
            for j in abertas_desconectar.borrow().sessoes.iter() {
                if j.get_monitor_unico() >= 0 && j.get_host() == s.get_host() {
                    let _ = j.hide();
                }
            }
        }
    });
    sessao.show().expect("mostrar sessão");
    abertas.borrow_mut().sessoes.push(sessao.clone_strong());
    sessao
}

fn abrir_pedido(tema: TemaEscolhido, abertas: &Rc<RefCell<Abertas>>) {
    let pedido = JanelaPedido::new().expect("janela do pedido");
    aplicar_tema(&pedido, tema);
    let fraca = pedido.as_weak();
    pedido.on_recusar(move || {
        if let Some(p) = fraca.upgrade() {
            let _ = p.hide();
        }
    });
    let fraca = pedido.as_weak();
    let abertas_permitir = abertas.clone();
    pedido.on_permitir(move || {
        let Some(p) = fraca.upgrade() else { return };
        let _ = p.hide();
        let faixa = FaixaHost::new().expect("faixa do host");
        aplicar_tema(&faixa, tema);
        faixa.set_origem(p.get_origem());
        faixa.set_arquivos(p.get_arquivos());
        let fraca_faixa = faixa.as_weak();
        faixa.on_encerrar(move || {
            if let Some(f) = fraca_faixa.upgrade() {
                let _ = f.hide();
            }
        });
        faixa.show().expect("mostrar faixa");
        abertas_permitir.borrow_mut().faixa = Some(faixa);
    });
    pedido.show().expect("mostrar pedido");
    abertas.borrow_mut().pedido = Some(pedido);
}

fn principal(tema: TemaEscolhido, secao: i32) -> Result<JanelaPrincipal, slint::PlatformError> {
    let janela = JanelaPrincipal::new()?;
    aplicar_tema(&janela, tema);
    janela.set_secao(secao);
    let todos = dispositivos_exemplo();
    janela.set_dispositivos(ModelRc::new(VecModel::from(todos.clone())));

    let fraca = janela.as_weak();
    janela.on_busca_mudou(move |busca: SharedString| {
        let Some(j) = fraca.upgrade() else { return };
        let termo = busca.to_lowercase();
        let filtrados: Vec<Dispositivo> = todos
            .iter()
            .filter(|d| {
                d.nome.to_lowercase().contains(&termo) || d.so.to_lowercase().contains(&termo)
            })
            .cloned()
            .collect();
        j.set_selecionado(0);
        j.set_dispositivos(ModelRc::new(VecModel::from(filtrados)));
    });

    let abertas = Rc::new(RefCell::new(Abertas::default()));
    let a = abertas.clone();
    janela.on_conectar(move |d| {
        abrir_sessao(&d, tema, &a);
    });
    let a = abertas.clone();
    janela.on_enviar_arquivo(move |d| {
        let s = abrir_sessao(&d, tema, &a);
        s.set_painel_arquivos(true);
    });
    let a = abertas.clone();
    janela.on_simular_pedido(move || abrir_pedido(tema, &a));
    // Mantém as janelas filhas vivas enquanto a principal existir.
    std::mem::forget(abertas);
    Ok(janela)
}

/// Sessão já conectada e sem animação, para capturas de tela.
fn sessao_estatica(
    tema: TemaEscolhido,
    etapa: Etapa,
) -> Result<JanelaSessao, slint::PlatformError> {
    let s = JanelaSessao::new()?;
    aplicar_tema(&s, tema);
    s.set_simular(false);
    s.set_etapa(etapa);
    Ok(s)
}

fn main() -> Result<(), slint::PlatformError> {
    let mut tela = String::from("principal");
    let mut tema = TemaEscolhido::Sistema;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tela" => tela = args.next().unwrap_or_default(),
            "--tema" => {
                tema = match args.next().as_deref() {
                    Some("escuro") => TemaEscolhido::Escuro,
                    Some("claro") => TemaEscolhido::Claro,
                    _ => TemaEscolhido::Sistema,
                }
            }
            _ => {
                eprintln!("uso: taildesk-prototipo [--tela <{TELAS}>] [--tema claro|escuro]");
                std::process::exit(2);
            }
        }
    }

    match tela.as_str() {
        "principal" => principal(tema, 0)?.run(),
        "transferencias" => principal(tema, 1)?.run(),
        "configuracoes" => principal(tema, 2)?.run(),
        "pedido" => {
            let p = JanelaPedido::new()?;
            aplicar_tema(&p, tema);
            p.run()
        }
        "faixa" => {
            let f = FaixaHost::new()?;
            aplicar_tema(&f, tema);
            f.run()
        }
        "sessao" => {
            let s = sessao_estatica(tema, Etapa::Conectada)?;
            s.set_toast("Teclado: ABNT2 → US-Intl (tradução automática)".into());
            s.run()
        }
        "sessao-monitores" => {
            let s = sessao_estatica(tema, Etapa::Conectada)?;
            let abrir = s.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(300), move || {
                if let Some(s) = abrir.upgrade() {
                    s.invoke_abrir_menu_monitores();
                }
            });
            s.run()
        }
        "sessao-wayland" => {
            let s = sessao_estatica(tema, Etapa::Conectada)?;
            s.set_host("servidor-homelab".into());
            s.set_teclado_host("ABNT2".into());
            s.set_host_wayland(true);
            let monitores = monitores_do("servidor-homelab");
            s.set_monitores_em_abas(2);
            s.set_monitores(ModelRc::new(VecModel::from(monitores)));
            let abrir = s.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(300), move || {
                if let Some(s) = abrir.upgrade() {
                    s.invoke_abrir_menu_monitores();
                }
            });
            s.run()
        }
        "sessao-com-janela" | "sessao-destacada" => {
            let s = sessao_estatica(tema, Etapa::Conectada)?;
            let mut monitores = monitores_do("desktop-casa");
            monitores[1].destacado = true;
            s.set_monitores(ModelRc::new(VecModel::from(monitores)));
            s.set_monitores_em_abas(1);
            if tela == "sessao-destacada" {
                s.set_monitor_unico(1);
            }
            s.run()
        }
        "sessao-arquivos" => {
            let s = sessao_estatica(tema, Etapa::Conectada)?;
            s.set_painel_arquivos(true);
            s.run()
        }
        "sessao-navegar" => {
            let s = sessao_estatica(tema, Etapa::Conectada)?;
            s.set_painel_arquivos(true);
            s.set_aba_arquivos(1);
            s.run()
        }
        "sessao-relay" => {
            let s = sessao_estatica(tema, Etapa::Conectada)?;
            s.set_via_relay(true);
            s.set_barra_fixada(false);
            s.set_toast("Conexão via relay do Tailscale. A imagem pode ficar mais lenta.".into());
            s.run()
        }
        "sessao-esperando" => sessao_estatica(tema, Etapa::Esperando)?.run(),
        "sessao-reconectando" => sessao_estatica(tema, Etapa::Reconectando)?.run(),
        outra => {
            eprintln!("tela desconhecida: {outra}. Opções: {TELAS}");
            std::process::exit(2);
        }
    }
}
