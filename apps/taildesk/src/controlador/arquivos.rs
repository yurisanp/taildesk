//! Painel de arquivos da janela da sessão (`12-levantamento-ux.md` §5.7): liga o
//! `PainelArquivos` do Slint ao serviço de arquivos da sessão (`Sessao::arquivos`), com a faixa
//! de transferências e o "Navegar no host" de `taildesk_files::panel`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use taildesk_files::audit::Direction;
use taildesk_files::listing::walk;
use taildesk_files::panel::fmt::{self, Locale};
use taildesk_files::panel::{Browser, ListRequest, Row, State, Transfers};
use taildesk_files::service::{Event, Files};
use taildesk_proto::control::{FileErrorCode, FsEntryKind};
use taildesk_proto::files::INBOX_ROOT;
use taildesk_session::ControllerState;
use taildesk_ui::{
    ArquivosSessao, ErroArquivo, EstadoTransferencia, ItemRemoto, JanelaSessao, LinhaTransferencia,
    RaizRemota,
};

use super::{IdLocal, SessaoUi, na_sessao, na_sessao_ja};

type Servico = Files<quinn::Connection>;

/// Progresso redesenha no máximo a cada tanto; mudanças de estado redesenham na hora.
const INTERVALO_PROGRESSO: Duration = Duration::from_millis(100);

/// Estado do painel de uma sessão (na thread da interface).
pub(super) struct Painel {
    id: IdLocal,
    servico: Option<Servico>,
    rt: tokio::runtime::Handle,
    transferencias: Transfers,
    navegador: Browser,
    locale: Locale,
    ultimo_desenho: Option<Instant>,
    /// Arrastar arquivos do sistema no Wayland (no Windows e no X11 vem pelo winit).
    #[cfg(target_os = "linux")]
    soltar: Option<taildesk_platform::soltar::Ouvinte>,
}

impl Painel {
    pub(super) fn new(id: IdLocal, rt: tokio::runtime::Handle) -> Self {
        Painel {
            id,
            servico: None,
            rt,
            transferencias: Transfers::new(),
            navegador: Browser::new(),
            locale: Locale::PtBr,
            ultimo_desenho: None,
            #[cfg(target_os = "linux")]
            soltar: None,
        }
    }

    /// Escuta arquivos arrastados para a janela no Wayland. `false` = a janela do winit ainda
    /// não existe (ela só nasce quando o laço de eventos processa o `show`).
    fn tentar_soltar(&mut self, janela: &JanelaSessao) -> bool {
        #[cfg(target_os = "linux")]
        {
            use slint::winit_030::WinitWindowAccessor;
            use slint::winit_030::winit::raw_window_handle::{
                HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
            };
            let id = self.id;
            let Some(ouvinte) = janela.window().with_winit_window(|w| {
                let (Ok(d), Ok(h)) = (w.display_handle(), w.window_handle()) else {
                    return None;
                };
                let (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(h)) =
                    (d.as_raw(), h.as_raw())
                else {
                    // X11: o winit entrega HoveredFile/DroppedFile.
                    return None;
                };
                // SAFETY: display e superfície vêm da janela viva do winit; o wl_display é
                // do event loop, que vive até o fim do app (mais que o painel).
                let ouvinte = unsafe {
                    taildesk_platform::soltar::ouvir_wayland(d.display, h.surface, move |s| {
                        soltura(id, s);
                    })
                };
                ouvinte
                    .inspect_err(|e| eprintln!("taildesk: arrastar arquivos indisponível: {e}"))
                    .ok()
            }) else {
                return false;
            };
            self.soltar = ouvinte;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = janela;
        true
    }

    /// Arquivos do sistema sobre a janela (mostra o aviso de soltar).
    pub(super) fn arrastando(&self, janela: &JanelaSessao, sim: bool) {
        janela.global::<ArquivosSessao>().set_arrastando(sim);
    }

    /// Arquivos (ou pastas) soltos na janela: vão para a pasta de entrada do host.
    pub(super) fn soltou(&mut self, janela: &JanelaSessao, caminhos: Vec<PathBuf>) {
        let g = janela.global::<ArquivosSessao>();
        g.set_arrastando(false);
        let Some(servico) = self.servico.clone().filter(|_| g.get_disponivel()) else {
            janela.set_toast("Esta sessão não recebe arquivos: o host não liberou.".into());
            return;
        };
        g.set_aba(0);
        g.set_aberto(true);
        self.rt.spawn(enviar(
            self.id,
            servico,
            INBOX_ROOT,
            String::new(),
            caminhos,
        ));
    }

    /// A sessão conectou: daqui em diante dá para enviar e baixar.
    pub(super) fn ligar(&mut self, servico: Servico) {
        self.servico = Some(servico);
    }

    /// Mostra o botão "Arquivos" só ao vivo e com a permissão de arquivos.
    pub(super) fn estado(&self, janela: &JanelaSessao, estado: &ControllerState) {
        let pode = self.servico.is_some()
            && matches!(estado, ControllerState::Live { permissions } if permissions.files);
        let g = janela.global::<ArquivosSessao>();
        g.set_disponivel(pode);
        if !pode {
            g.set_aberto(false);
        }
    }

    pub(super) fn evento(&mut self, janela: &JanelaSessao, ev: Event) {
        let agora = Instant::now();
        match &ev {
            Event::Roots(r) => {
                self.navegador.set_roots(r);
                self.desenhar_navegador(janela);
            }
            Event::Listing(l) => {
                if let Some(proxima) = self.navegador.apply(l) {
                    self.pedir(proxima);
                }
                self.desenhar_navegador(janela);
            }
            Event::Progress { .. } => {
                if self.transferencias.apply(&ev, agora)
                    && self
                        .ultimo_desenho
                        .is_none_or(|t| agora.duration_since(t) >= INTERVALO_PROGRESSO)
                {
                    self.desenhar_transferencias(janela);
                }
            }
            Event::Finished {
                direction, path, ..
            } => {
                let nome = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if self.transferencias.apply(&ev, agora) && *direction == Direction::In {
                    janela.set_toast(format!("{nome} salvo em {}.", pasta_de(path)).into());
                }
                self.desenhar_transferencias(janela);
            }
            Event::Receiving { .. } | Event::Failed { .. } => {
                self.transferencias.apply(&ev, agora);
                self.desenhar_transferencias(janela);
            }
        }
    }

    /// Pede uma página ao host; o `req_id` volta para o navegador.
    fn pedir(&mut self, req: ListRequest) {
        let Some(servico) = self.servico.clone() else {
            return;
        };
        let id = self.id;
        self.rt.spawn(async move {
            let req_id = servico.list(req.root, &req.path, req.start).await;
            na_sessao(id, move |s| s.painel.navegador.requested(req_id));
        });
    }

    fn linha(&self, r: &Row) -> LinhaTransferencia {
        let l = self.locale;
        let (estado, erro, retomavel) = match &r.state {
            State::Waiting => (EstadoTransferencia::Esperando, ErroArquivo::Outro, false),
            State::Running => (EstadoTransferencia::Andamento, ErroArquivo::Outro, false),
            State::Done { .. } => (EstadoTransferencia::Concluida, ErroArquivo::Outro, false),
            State::Cancelled => (EstadoTransferencia::Cancelada, ErroArquivo::Outro, false),
            State::Failed { code, resumable } => {
                (EstadoTransferencia::Falhou, erro_de(*code), *resumable)
            }
        };
        let (destino, nome_final, renomeado) = match &r.state {
            State::Done { path, renamed } if r.direction == Direction::In => (
                pasta_de(path),
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                *renamed,
            ),
            _ => (String::new(), String::new(), false),
        };
        LinhaTransferencia {
            id: r.xfer_id as i32,
            nome: r.name.as_str().into(),
            enviando: r.direction == Direction::Out,
            estado,
            progresso: r.fraction(),
            tamanho: fmt::size(r.size, l).into(),
            feito: fmt::size(r.done, l).into(),
            velocidade: if r.rate > 0.0 {
                fmt::rate(r.rate, l).into()
            } else {
                SharedString::new()
            },
            restante: r.eta().map(fmt::eta).unwrap_or_default().into(),
            destino: destino.into(),
            renomeado,
            nome_final: nome_final.into(),
            erro,
            retomavel,
        }
    }

    fn desenhar_transferencias(&mut self, janela: &JanelaSessao) {
        self.ultimo_desenho = Some(Instant::now());
        // Mais recentes primeiro.
        let linhas: Vec<_> = self
            .transferencias
            .rows()
            .iter()
            .rev()
            .map(|r| self.linha(r))
            .collect();
        let g = janela.global::<ArquivosSessao>();
        g.set_ativas(self.transferencias.active() as i32);
        g.set_transferencias(ModelRc::new(VecModel::from(linhas)));
    }

    fn desenhar_navegador(&self, janela: &JanelaSessao) {
        let l = self.locale;
        let n = &self.navegador;
        let g = janela.global::<ArquivosSessao>();
        let raizes: Vec<_> = n
            .roots()
            .iter()
            .map(|r| RaizRemota {
                id: r.id as i32,
                nome: r.name.as_str().into(),
                permitida: r.readable,
            })
            .collect();
        g.set_raizes(ModelRc::new(VecModel::from(raizes)));
        g.set_raiz_aberta(n.root().map(|r| r.name.as_str()).unwrap_or("").into());
        g.set_pode_enviar_aqui(n.root().is_some_and(|r| r.writable));
        let caminho: Vec<SharedString> = n.dirs().iter().map(|d| d.as_str().into()).collect();
        g.set_caminho(ModelRc::new(VecModel::from(caminho)));
        let itens: Vec<_> = n
            .entries()
            .iter()
            .filter(|e| e.kind != FsEntryKind::Other as i32)
            .map(|e| {
                let pasta = e.kind == FsEntryKind::Dir as i32;
                ItemRemoto {
                    nome: e.name.as_str().into(),
                    pasta,
                    tamanho: if pasta {
                        SharedString::new()
                    } else {
                        fmt::size(e.size, l).into()
                    },
                    data: fmt::date(e.mtime_ms, l).into(),
                }
            })
            .collect();
        g.set_itens(ModelRc::new(VecModel::from(itens)));
        g.set_carregando(n.loading());
        g.set_tem_erro(n.error().is_some());
        g.set_erro(n.error().map_or(ErroArquivo::Outro, erro_de));
    }
}

/// Roda `f` no painel da sessão `id`, já na thread da interface.
fn no_painel(id: IdLocal, f: impl FnOnce(&mut Painel, &JanelaSessao)) {
    na_sessao_ja(id, |s: &mut SessaoUi| f(&mut s.painel, &s.janela));
}

fn erro_de(code: FileErrorCode) -> ErroArquivo {
    match code {
        FileErrorCode::Denied => ErroArquivo::SemPermissao,
        FileErrorCode::NotFound => ErroArquivo::NaoEncontrado,
        FileErrorCode::BadPath => ErroArquivo::Caminho,
        FileErrorCode::NoSpace => ErroArquivo::SemEspaco,
        FileErrorCode::HashMismatch => ErroArquivo::Integridade,
        FileErrorCode::Busy => ErroArquivo::Ocupado,
        FileErrorCode::Changed => ErroArquivo::Mudou,
        FileErrorCode::Io | FileErrorCode::ResumeMismatch => ErroArquivo::Conexao,
        _ => ErroArquivo::Outro,
    }
}

fn pasta_de(path: &Path) -> String {
    path.parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// Onde ficam os downloads do host: `Downloads/TailDesk`, como a pasta de entrada do host.
fn pasta_de_downloads() -> Option<PathBuf> {
    taildesk_platform::pasta_downloads().map(|d| d.join("TailDesk"))
}

/// Envia `escolhidos` (arquivos e pastas) para `root`, em `base` (`""` = a raiz).
async fn enviar(id: IdLocal, servico: Servico, root: u32, base: String, escolhidos: Vec<PathBuf>) {
    let destino = |rel: &str| {
        if base.is_empty() {
            rel.to_owned()
        } else {
            format!("{base}/{rel}")
        }
    };
    for p in escolhidos {
        let arquivos = if p.is_dir() {
            let dir = p.clone();
            match tokio::task::spawn_blocking(move || walk(&dir)).await {
                Ok(Ok(w)) => w
                    .files
                    .into_iter()
                    .map(|f| (f.path, f.rel_path, f.size))
                    .collect(),
                _ => {
                    aviso(id, format!("Não foi possível ler a pasta {}.", p.display()));
                    continue;
                }
            }
        } else {
            let Some(nome) = p.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                aviso(id, format!("Nome não aceito: {}.", p.display()));
                continue;
            };
            let tamanho = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            vec![(p, nome, tamanho)]
        };
        for (caminho, rel, tamanho) in arquivos {
            let nome = rel.rsplit('/').next().unwrap_or(&rel).to_owned();
            // Espera uma vaga (até 4 envios ao mesmo tempo) antes de virar linha.
            match servico.send_file(&caminho, root, &destino(&rel)).await {
                Ok(xfer_id) => no_app_painel(id, move |p, j| {
                    p.transferencias.sending(xfer_id, nome, tamanho);
                    p.desenhar_transferencias(j);
                }),
                Err(e) => aviso(id, format!("Não foi possível enviar {nome}: {e}.")),
            }
        }
    }
}

/// Liga o arrastar de arquivos da janela da sessão `id` assim que a janela do winit existir
/// (tenta a cada 100 ms, até `tentativas` vezes).
pub(super) fn ligar_soltar(id: IdLocal, tentativas: u8) {
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let pronto = na_sessao_ja(id, |s| s.painel.tentar_soltar(&s.janela)).unwrap_or(true);
        if !pronto && tentativas > 1 {
            ligar_soltar(id, tentativas - 1);
        }
    });
}

/// Evento do ouvinte do Wayland (na thread dele).
#[cfg(target_os = "linux")]
fn soltura(id: IdLocal, s: taildesk_platform::soltar::Soltura) {
    use taildesk_platform::soltar::Soltura;
    no_app_painel(id, move |p, j| match s {
        Soltura::Entrou => p.arrastando(j, true),
        Soltura::Saiu => p.arrastando(j, false),
        Soltura::Soltou(caminhos) => p.soltou(j, caminhos),
    });
}

fn no_app_painel(id: IdLocal, f: impl FnOnce(&mut Painel, &JanelaSessao) + Send + 'static) {
    na_sessao(id, move |s| f(&mut s.painel, &s.janela));
}

fn aviso(id: IdLocal, texto: String) {
    na_sessao(id, move |s| s.janela.set_toast(texto.into()));
}

/// Escolhe arquivos (ou pastas) no diálogo do sistema e envia para `root`/`base`.
fn escolher_e_enviar(id: IdLocal, pastas: bool, root: u32, base: String) {
    no_painel(id, |p, _| {
        let Some(servico) = p.servico.clone() else {
            return;
        };
        p.rt.spawn(async move {
            let titulo = if pastas {
                "Enviar pasta"
            } else {
                "Enviar arquivos"
            };
            match taildesk_platform::arquivos::escolher(titulo, pastas).await {
                Ok(escolhidos) if !escolhidos.is_empty() => {
                    enviar(id, servico, root, base, escolhidos).await;
                }
                Ok(_) => {}
                Err(e) => aviso(
                    id,
                    format!("Não foi possível abrir a escolha de arquivos: {e}."),
                ),
            }
        });
    });
}

/// Liga os callbacks do painel na janela da sessão `id`.
pub(super) fn ligar_callbacks(janela: &JanelaSessao, id: IdLocal) {
    let g = janela.global::<ArquivosSessao>();
    g.on_cancelar(move |xfer| {
        no_painel(id, |p, j| {
            let xfer = xfer as u32;
            if let Some(s) = p.servico.clone() {
                p.rt.spawn(async move { s.cancel(xfer).await });
            }
            p.transferencias.cancelled(xfer);
            p.desenhar_transferencias(j);
        });
    });
    g.on_abrir_pasta(move |xfer| {
        no_painel(id, |p, _| {
            let Some(Row {
                state: State::Done { path, .. },
                ..
            }) = p.transferencias.get(xfer as u32)
            else {
                return;
            };
            let Some(dir) = path.parent().map(Path::to_path_buf) else {
                return;
            };
            p.rt.spawn(async move {
                if let Err(e) = taildesk_platform::arquivos::abrir_pasta(&dir).await {
                    aviso(
                        id,
                        format!("Não foi possível abrir {}: {e}.", dir.display()),
                    );
                }
            });
        });
    });
    g.on_limpar(move || {
        no_painel(id, |p, j| {
            p.transferencias.clear_finished();
            p.desenhar_transferencias(j);
        });
    });
    g.on_enviar_arquivos(move || escolher_e_enviar(id, false, INBOX_ROOT, String::new()));
    g.on_enviar_pasta(move || escolher_e_enviar(id, true, INBOX_ROOT, String::new()));
    g.on_enviar_aqui(move || {
        let alvo = na_sessao_ja(id, |s| {
            let n = &s.painel.navegador;
            n.root().filter(|r| r.writable).map(|r| (r.id, n.path()))
        })
        .flatten();
        if let Some((root, base)) = alvo {
            escolher_e_enviar(id, false, root, base);
        }
    });
    g.on_listar_raizes(move || {
        no_painel(id, |p, _| {
            if let Some(s) = p.servico.clone() {
                p.rt.spawn(async move { s.request_roots().await });
            }
        });
    });
    g.on_abrir_raiz(move |raiz| {
        no_painel(id, |p, j| {
            if let Some(req) = p.navegador.open_root(raiz as u32) {
                p.pedir(req);
            }
            p.desenhar_navegador(j);
        });
    });
    g.on_entrar(move |nome| {
        no_painel(id, |p, j| {
            if let Some(req) = p.navegador.open_dir(&nome) {
                p.pedir(req);
            }
            p.desenhar_navegador(j);
        });
    });
    g.on_ir_para(move |nivel| {
        no_painel(id, |p, j| {
            match usize::try_from(nivel) {
                Ok(n) => {
                    if let Some(req) = p.navegador.go_to(n) {
                        p.pedir(req);
                    }
                }
                Err(_) => {
                    p.navegador.close_root();
                    if let Some(s) = p.servico.clone() {
                        p.rt.spawn(async move { s.request_roots().await });
                    }
                }
            }
            p.desenhar_navegador(j);
        });
    });
    g.on_atualizar(move || {
        no_painel(id, |p, j| {
            if let Some(req) = p.navegador.refresh() {
                p.pedir(req);
            }
            p.desenhar_navegador(j);
        });
    });
    g.on_baixar(move |nome| {
        no_painel(id, |p, j| {
            let Some(servico) = p.servico.clone() else {
                return;
            };
            let Some(root) = p.navegador.root().map(|r| r.id) else {
                return;
            };
            let Some(destino) = pasta_de_downloads() else {
                j.set_toast("Não foi possível achar a pasta de downloads.".into());
                return;
            };
            let caminho = p.navegador.item_path(&nome);
            let nome = nome.to_string();
            p.rt.spawn(async move {
                if let Err(e) = tokio::fs::create_dir_all(&destino).await {
                    return aviso(
                        id,
                        format!("Não foi possível criar {}: {e}.", destino.display()),
                    );
                }
                let xfer_id = servico.download(root, &caminho, &destino).await;
                no_app_painel(id, move |p, j| {
                    p.transferencias.downloading(xfer_id, nome);
                    p.desenhar_transferencias(j);
                });
            });
        });
    });
}
