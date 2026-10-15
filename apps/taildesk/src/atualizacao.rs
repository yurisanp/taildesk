//! Atualizador do app (`docs/context/16-atualizacao-automatica.md` §4 e §5): procura versão nova ao
//! abrir e a cada 24 h, mostra a faixa na tela inicial e o cartão em Configurações → Atualizações,
//! baixa e troca o AppImage e reinicia quando o usuário pedir.
//!
//! A rede e o download rodam numa thread própria ([`trabalhador`]); o estado vive na thread da
//! interface. Nunca instala nem reinicia com sessão ativa (host ou controlador): "Atualizar agora"
//! durante uma sessão fica guardado e começa quando ela terminar.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use slint::{SharedString, Timer, TimerMode};
use taildesk_ui::{EstadoAtualizacao, InfoAtualizacao, JanelaPrincipal};
use taildesk_update::{Atualizacao, Canal, Erro, Instalacao};

use crate::config::{self, PrefsAtualizacao};
use crate::{App, avisar, com_app, no_app};

/// De quanto em quanto tempo procura de novo, com a procura automática ligada.
const INTERVALO: Duration = Duration::from_secs(24 * 60 * 60);
/// De quanto em quanto tempo confere se a sessão que adiou a instalação já terminou.
const CONFERIR_SESSAO: Duration = Duration::from_secs(5);

enum Pedido {
    Procurar,
    Prefs(PrefsAtualizacao),
    Instalar(Box<Atualizacao>, PathBuf),
}

struct Estado {
    prefs: PrefsAtualizacao,
    instalacao: Instalacao,
    automatica: bool,
    info: InfoAtualizacao,
    nova: Option<Atualizacao>,
    /// Versão que o usuário mandou para "Depois": a faixa volta só com uma mais nova.
    dispensada: Option<String>,
    /// "Atualizar agora" durante uma sessão: instala quando ela terminar.
    pendente: bool,
    tx: Sender<Pedido>,
    _timer: Timer,
}

thread_local! {
    static ESTADO: RefCell<Option<Estado>> = const { RefCell::new(None) };
    /// AppImage para `exec` depois que o event loop terminar ([`reiniciar_se_pedido`]).
    static REINICIAR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn com_estado<R>(f: impl FnOnce(&mut Estado) -> R) -> Option<R> {
    ESTADO.with(|e| e.borrow_mut().as_mut().map(f))
}

/// Começa a procurar atualizações. Chamar na thread da interface, com o app já criado.
pub fn iniciar(prefs: PrefsAtualizacao) {
    let instalacao = Instalacao::detectar();
    let automatica = instalacao.automatica();
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("taildesk-atualizacao".into())
        .spawn(move || trabalhador(rx, prefs));
    if let Err(e) = thread {
        eprintln!("taildesk: atualizador indisponível: {e}");
        return;
    }
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, CONFERIR_SESSAO, conferir_sessao);
    let info = InfoAtualizacao {
        instalacao: match instalacao {
            Instalacao::AppImage { .. } => taildesk_ui::Instalacao::Appimage,
            Instalacao::PacoteDistro => taildesk_ui::Instalacao::Pacote,
            Instalacao::Arquivo => taildesk_ui::Instalacao::Arquivo,
        },
        automatica,
        ..Default::default()
    };
    ESTADO.with(|e| {
        *e.borrow_mut() = Some(Estado {
            prefs,
            instalacao,
            automatica,
            info,
            nova: None,
            dispensada: None,
            pendente: false,
            tx,
            _timer: timer,
        })
    });
    com_app(|app| {
        if let Some(b) = &app.bandeja {
            b.on_abrir_atualizacao(|| {
                no_app(|app| {
                    app.mostrar(None);
                    if let Some(j) = app.janela.borrow().as_ref() {
                        j.set_secao(1);
                    }
                })
            });
        }
    });
}

/// Preenche uma janela recém-criada e liga os callbacks.
pub fn ligar(j: &JanelaPrincipal) {
    com_estado(|e| {
        j.set_atualizacao(e.info.clone());
        j.set_atualizar_auto(e.prefs.automatica);
        j.set_canal_pre(e.prefs.pre);
    });
    j.on_atualizar_auto_mudou(|v| mudar_prefs(|p| p.automatica = v));
    j.on_canal_pre_mudou(|v| mudar_prefs(|p| p.pre = v));
    j.on_procurar_atualizacao(|| {
        com_estado(|e| {
            let _ = e.tx.send(Pedido::Procurar);
        });
    });
    j.on_atualizar_agora(atualizar_agora);
    j.on_reiniciar_app(reiniciar_app);
    j.on_dispensar_atualizacao(|| {
        mudar(|e| e.dispensada = e.nova.as_ref().map(|n| n.versao.to_string()));
    });
}

/// Muda o estado e mostra o resultado na janela e na bandeja.
fn mudar(f: impl FnOnce(&mut Estado)) {
    let Some((info, bandeja)) = com_estado(|e| {
        f(e);
        let i = &mut e.info;
        let versao = i.versao.to_string();
        let dispensada = e.dispensada.as_deref() == Some(versao.as_str());
        i.aviso = match i.estado {
            EstadoAtualizacao::Disponivel => !dispensada,
            EstadoAtualizacao::Baixando | EstadoAtualizacao::Pronta => true,
            // Falha ao instalar o que o usuário pediu: a faixa mostra o erro e "Tentar de novo".
            EstadoAtualizacao::Erro => e.nova.is_some() && !dispensada,
            _ => false,
        };
        let bandeja = match i.estado {
            EstadoAtualizacao::Disponivel | EstadoAtualizacao::Baixando => {
                format!("TailDesk {versao} disponível")
            }
            EstadoAtualizacao::Pronta => format!("TailDesk {versao}: reinicie para concluir"),
            _ => String::new(),
        };
        (i.clone(), bandeja)
    }) else {
        return;
    };
    com_app(|app| {
        if let Some(j) = app.janela.borrow().as_ref() {
            j.set_atualizacao(info);
        }
        if let Some(b) = &app.bandeja {
            b.set_atualizacao(bandeja.into());
        }
    });
}

fn mudar_prefs(f: impl FnOnce(&mut PrefsAtualizacao)) {
    let Some(prefs) = com_estado(|e| {
        f(&mut e.prefs);
        let _ = e.tx.send(Pedido::Prefs(e.prefs));
        e.prefs
    }) else {
        return;
    };
    let mut c = config::ler();
    c.atualizacao = prefs;
    if let Err(e) = config::gravar(&c) {
        eprintln!("taildesk: não foi possível gravar a configuração: {e}");
    }
}

fn sessao_ativa() -> bool {
    com_app(App::sessao_ativa).unwrap_or(false)
}

fn atualizar_agora() {
    if sessao_ativa() {
        mudar(|e| {
            e.pendente = true;
            e.info.adiada = true;
        });
        return;
    }
    instalar();
}

/// Começa o download, se houver versão nova e o app puder se trocar sozinho.
fn instalar() {
    mudar(|e| {
        e.pendente = false;
        let (Some(nova), Instalacao::AppImage { caminho }, true) =
            (&e.nova, &e.instalacao, e.automatica)
        else {
            return;
        };
        if e.info.estado != EstadoAtualizacao::Disponivel {
            return;
        }
        let pedido = Pedido::Instalar(Box::new(nova.clone()), caminho.clone());
        if e.tx.send(pedido).is_ok() {
            e.info.estado = EstadoAtualizacao::Baixando;
            e.info.progresso = 0.0;
        }
    });
}

fn reiniciar_app() {
    if sessao_ativa() {
        avisar("Reinicie o TailDesk quando a sessão terminar.".into());
        return;
    }
    let caminho = com_estado(|e| match (&e.instalacao, e.info.estado) {
        (Instalacao::AppImage { caminho }, EstadoAtualizacao::Pronta) => Some(caminho.clone()),
        _ => None,
    })
    .flatten();
    if let Some(c) = caminho {
        REINICIAR.with(|r| *r.borrow_mut() = Some(c));
        let _ = slint::quit_event_loop();
    }
}

/// Depois do event loop: troca o processo pelo AppImage novo, se o usuário pediu para reiniciar.
pub fn reiniciar_se_pedido() {
    let Some(caminho) = REINICIAR.with(|r| r.borrow_mut().take()) else {
        return;
    };
    #[cfg(unix)]
    {
        let e = taildesk_update::reiniciar(&caminho);
        eprintln!(
            "taildesk: não foi possível reiniciar ({e}); abra {} de novo",
            caminho.display()
        );
    }
    #[cfg(not(unix))]
    let _ = caminho;
}

/// Atualiza "adiada" e, quando a sessão termina, começa a instalação que ficou esperando.
fn conferir_sessao() {
    let ativa = sessao_ativa();
    let Some((mudou, pendente)) = com_estado(|e| (e.info.adiada != ativa, e.pendente)) else {
        return;
    };
    if mudou {
        mudar(|e| e.info.adiada = ativa);
    }
    if pendente && !ativa {
        instalar();
    }
}

/// Baixando ou já trocou o arquivo: a versão em execução continua a antiga até reiniciar, então
/// uma procura nova acharia a mesma versão de novo.
fn ocupado(e: &Estado) -> bool {
    matches!(
        e.info.estado,
        EstadoAtualizacao::Pronta | EstadoAtualizacao::Baixando
    )
}

fn resultado(r: Result<Option<Atualizacao>, Erro>) {
    mudar(|e| {
        if ocupado(e) {
            return;
        }
        match r {
            Ok(Some(a)) => {
                e.info.estado = EstadoAtualizacao::Disponivel;
                e.info.versao = a.versao.to_string().into();
                e.info.pagina = a.pagina.as_str().into();
                e.nova = Some(a);
            }
            Ok(None) => {
                e.info.estado = EstadoAtualizacao::EmDia;
                e.nova = None;
            }
            // Falha de rede numa procura nova: continua mostrando a versão que já tinha achado.
            Err(_) if e.nova.is_some() => e.info.estado = EstadoAtualizacao::Disponivel,
            Err(erro) => {
                e.info.estado = EstadoAtualizacao::Erro;
                e.info.erro = SharedString::from(erro.to_string());
            }
        }
    });
}

fn instalado(r: Result<(), Erro>) {
    mudar(|e| match r {
        Ok(()) => e.info.estado = EstadoAtualizacao::Pronta,
        Err(erro) => {
            e.info.estado = EstadoAtualizacao::Erro;
            e.info.erro = SharedString::from(erro.to_string());
        }
    });
}

/// Thread da rede: procura ao abrir e a cada [`INTERVALO`] (com a procura automática ligada),
/// quando o usuário pede e quando ele muda o canal; baixa e troca o AppImage.
fn trabalhador(rx: Receiver<Pedido>, mut prefs: PrefsAtualizacao) {
    let mut proxima = prefs.automatica.then(Instant::now);
    loop {
        let pedido = match proxima {
            Some(t) => match rx.recv_timeout(t.saturating_duration_since(Instant::now())) {
                Ok(p) => p,
                Err(RecvTimeoutError::Timeout) => Pedido::Procurar,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(p) => p,
                Err(_) => return,
            },
        };
        match pedido {
            Pedido::Prefs(p) => {
                prefs = p;
                proxima = prefs.automatica.then(Instant::now);
            }
            Pedido::Procurar => {
                no_app(|_| {
                    mudar(|e| {
                        if !ocupado(e) {
                            e.info.estado = EstadoAtualizacao::Procurando;
                        }
                    })
                });
                let canal = if prefs.pre {
                    Canal::PreLancamento
                } else {
                    Canal::Estavel
                };
                let r = taildesk_update::procurar(canal, &taildesk_update::versao_atual());
                no_app(move |_| resultado(r));
                proxima = prefs.automatica.then(|| Instant::now() + INTERVALO);
            }
            Pedido::Instalar(a, caminho) => {
                let mut ultimo = 0u64;
                let r = taildesk_update::instalar_appimage(&a, &caminho, |baixados, total| {
                    // No máximo 200 atualizações da barra.
                    let passo = (baixados * 200).checked_div(total).unwrap_or(0);
                    if passo != ultimo {
                        ultimo = passo;
                        let p = baixados as f32 / total as f32;
                        no_app(move |_| mudar(|e| e.info.progresso = p));
                    }
                });
                no_app(move |_| instalado(r));
            }
        }
    }
}
