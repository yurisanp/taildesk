//! Área de transferência da janela da sessão: liga o backend da janela à ponte da sessão
//! quando a sessão fica ao vivo com `Permissions.clipboard`.
//!
//! As mensagens 70–73 do host não passam pela thread da interface: a task da sessão
//! ([`super::rodar`]) as entrega direto à ponte pelo [`Entrada`], na ordem em que chegam.

use std::sync::{Arc, Mutex, PoisonError};

use taildesk_proto::control::{Control, Msg};
use taildesk_session::conexao::clipboard::{ArquivosDaArea, Ponte};
use taildesk_ui::JanelaSessao;
use tokio::sync::mpsc;

/// Onde a task da sessão entrega as mensagens da área de transferência (vazio = sem área).
#[derive(Clone, Debug, Default)]
pub(super) struct Entrada(Arc<Mutex<Option<mpsc::Sender<Msg>>>>);

impl Entrada {
    pub(super) fn atual(&self) -> Option<mpsc::Sender<Msg>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn trocar(&self, tx: Option<mpsc::Sender<Msg>>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = tx;
    }
}

/// Mensagem da área de transferência (vai para a ponte, não para a janela).
pub(super) fn e_da_area(m: &Msg) -> bool {
    matches!(
        m,
        Msg::ClipboardOffer(_)
            | Msg::ClipboardRequest(_)
            | Msg::ClipboardData(_)
            | Msg::ClipboardError(_)
    )
}

/// A área de transferência de uma janela de sessão.
#[derive(Default)]
pub(super) struct Area {
    entrada: Entrada,
    ponte: Option<Ponte>,
    /// Serviço de arquivos da sessão, para copiar e colar arquivos.
    arquivos: Option<Arc<dyn ArquivosDaArea>>,
}

impl std::fmt::Debug for Area {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Area")
            .field("ponte", &self.ponte)
            .field("arquivos", &self.arquivos.is_some())
            .finish_non_exhaustive()
    }
}

impl Area {
    pub(super) fn new(entrada: Entrada) -> Area {
        Area {
            entrada,
            ponte: None,
            arquivos: None,
        }
    }

    /// O serviço de arquivos da sessão (antes de ficar ao vivo).
    pub(super) fn arquivos(&mut self, a: Arc<dyn ArquivosDaArea>) {
        self.arquivos = Some(a);
    }

    /// Liga (ao ficar ao vivo com a permissão). Sem backend neste sistema, os pedidos do host
    /// voltam `UNAVAILABLE` pela task da sessão.
    pub(super) fn ligar(
        &mut self,
        janela: &JanelaSessao,
        rt: &tokio::runtime::Handle,
        saida: mpsc::Sender<Control>,
    ) {
        if self.ponte.is_some() {
            return;
        }
        let _dentro = rt.enter();
        let Some(ponte) = ponte(janela, saida, self.arquivos.clone()) else {
            return;
        };
        self.entrada.trocar(Some(ponte.entrada()));
        self.ponte = Some(ponte);
    }

    /// A sessão acabou.
    pub(super) fn desligar(&mut self) {
        self.entrada.trocar(None);
        self.ponte = None;
    }
}

type Arquivos = Option<Arc<dyn ArquivosDaArea>>;

#[cfg(target_os = "linux")]
fn ponte(janela: &JanelaSessao, saida: mpsc::Sender<Control>, arq: Arquivos) -> Option<Ponte> {
    use slint::ComponentHandle;
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    let (area, rx) = janela
        .window()
        .with_winit_window(|w| {
            let RawDisplayHandle::Wayland(d) = w.display_handle().ok()?.as_raw() else {
                // X11: ainda sem área de transferência.
                return None;
            };
            // SAFETY: o wl_display é o do event loop do winit, que vive até o fim do app (mais
            // que a janela da sessão e a área dela).
            unsafe { crate::area::janela_wayland(d.display) }
                .inspect_err(|e| eprintln!("taildesk: área de transferência indisponível: {e}"))
                .ok()
        })
        .flatten()?;
    Some(Ponte::com_arquivos(area, rx, saida, true, arq))
}

#[cfg(windows)]
fn ponte(_: &JanelaSessao, saida: mpsc::Sender<Control>, arq: Arquivos) -> Option<Ponte> {
    let (area, rx) = crate::area::windows()
        .inspect_err(|e| eprintln!("taildesk: área de transferência indisponível: {e}"))
        .ok()?;
    Some(Ponte::com_arquivos(area, rx, saida, true, arq))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn ponte(_: &JanelaSessao, _: mpsc::Sender<Control>, _: Arquivos) -> Option<Ponte> {
    None
}
