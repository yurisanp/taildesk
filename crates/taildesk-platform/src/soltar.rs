//! Arrastar arquivos do sistema para a janela da sessão.
//!
//! - Windows e X11: o winit já entrega `HoveredFile`/`DroppedFile`; quem trata é o app.
//! - Wayland: o winit não implementa arrastar-e-soltar. Aqui uma conexão "convidada" no mesmo
//!   `wl_display` da janela (fila própria, numa thread) escuta o `wl_data_device` de cada
//!   `wl_seat` e aceita `text/uri-list` quando o arrasto entra na superfície da janela.

use std::path::PathBuf;

/// O que acontece com um arrasto de arquivos sobre a janela.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Soltura {
    /// Arquivos entraram na janela (ainda não soltos).
    Entrou,
    /// O arrasto saiu da janela ou foi cancelado.
    Saiu,
    /// Arquivos soltos na janela.
    Soltou(Vec<PathBuf>),
}

/// Caminhos locais de um `text/uri-list` (RFC 2483): uma URI por linha, `#` é comentário.
pub fn caminhos_de_uri_list(texto: &str) -> Vec<PathBuf> {
    texto
        .lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(crate::arquivos::caminho_de_uri)
        .collect()
}

#[cfg(target_os = "linux")]
pub use wayland::{Ouvinte, ouvir_wayland};

#[cfg(target_os = "linux")]
mod wayland {
    use std::ffi::c_void;
    use std::io::{self, Read};
    use std::os::fd::AsFd;
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use wayland_client::backend::{Backend, ObjectId, WaylandError};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
    use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
    use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::protocol::wl_seat::WlSeat;
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::{
        Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child,
    };

    use super::{Soltura, caminhos_de_uri_list};

    const URI_LIST: &str = "text/uri-list";
    /// Quanto esperar o app de origem escrever a lista depois do drop.
    const LEITURA_MAX: Duration = Duration::from_secs(5);

    /// Escuta enquanto existir; ao cair, para a thread e solta os objetos Wayland dela.
    #[derive(Debug)]
    pub struct Ouvinte {
        parar: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for Ouvinte {
        fn drop(&mut self) {
            self.parar.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    /// Ponteiro da libwayland que atravessa para a thread do ouvinte.
    struct Ptr(NonNull<c_void>);
    // SAFETY: a libwayland-client é thread-safe para conexões com filas separadas; o ponteiro
    // só é usado para criar a conexão convidada e o id da superfície (comparado, nunca usado
    // para enviar pedidos).
    unsafe impl Send for Ptr {}

    /// Começa a escutar arrastos de arquivos sobre `surface`. `aviso` roda na thread do ouvinte.
    ///
    /// # Safety
    ///
    /// `display` e `surface` têm de ser o `wl_display` e o `wl_surface` vivos da janela (do
    /// `raw-window-handle`), e o `wl_display` tem de viver mais que o [`Ouvinte`].
    pub unsafe fn ouvir_wayland(
        display: NonNull<c_void>,
        surface: NonNull<c_void>,
        aviso: impl FnMut(Soltura) + Send + 'static,
    ) -> io::Result<Ouvinte> {
        let parar = Arc::new(AtomicBool::new(false));
        let (pronto_tx, pronto_rx) = std::sync::mpsc::sync_channel(1);
        let (d, s) = (Ptr(display), Ptr(surface));
        let p = parar.clone();
        let thread = std::thread::Builder::new()
            .name("taildesk-soltar".into())
            .spawn(move || {
                let (d, s) = (d, s);
                // SAFETY: repassado do contrato de `ouvir_wayland`.
                match unsafe { iniciar(d.0, s.0, Box::new(aviso)) } {
                    Ok((fila, estado)) => {
                        let _ = pronto_tx.send(Ok(()));
                        rodar(fila, estado, &p);
                    }
                    Err(e) => {
                        let _ = pronto_tx.send(Err(e));
                    }
                }
            })?;
        pronto_rx
            .recv()
            .map_err(|_| io::Error::other("ouvinte de arrastar terminou"))??;
        Ok(Ouvinte {
            parar,
            thread: Some(thread),
        })
    }

    struct Estado {
        conexao: Connection,
        superficie: ObjectId,
        gerente: WlDataDeviceManager,
        dispositivos: Vec<WlDataDevice>,
        /// Oferta do arrasto atual sobre a nossa superfície.
        atual: Option<WlDataOffer>,
        aviso: Box<dyn FnMut(Soltura) + Send>,
    }

    /// Tipos MIME anunciados por uma oferta.
    #[derive(Default)]
    struct Tipos(std::sync::Mutex<Vec<String>>);

    impl Tipos {
        fn tem(&self, mime: &str) -> bool {
            self.0.lock().is_ok_and(|v| v.iter().any(|m| m == mime))
        }
    }

    unsafe fn iniciar(
        display: NonNull<c_void>,
        surface: NonNull<c_void>,
        aviso: Box<dyn FnMut(Soltura) + Send>,
    ) -> io::Result<(EventQueue<Estado>, Estado)> {
        // SAFETY: `display` é o `wl_display` vivo da janela (contrato de `ouvir_wayland`).
        let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
        let conexao = Connection::from_backend(backend);
        // SAFETY: `surface` é um `wl_surface` vivo; o id só é comparado com os do `enter`.
        let superficie =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }
                .map_err(|_| io::Error::other("a superfície não é um wl_surface"))?;
        let (globais, mut fila) =
            registry_queue_init::<Estado>(&conexao).map_err(io::Error::other)?;
        let qh = fila.handle();
        let gerente: WlDataDeviceManager = globais
            .bind(&qh, 1..=3, ())
            .map_err(|_| io::Error::from(io::ErrorKind::Unsupported))?;
        let mut estado = Estado {
            conexao,
            superficie,
            gerente,
            dispositivos: Vec::new(),
            atual: None,
            aviso,
        };
        for g in globais.contents().clone_list() {
            if g.interface == WlSeat::interface().name {
                let seat: WlSeat = globais.registry().bind(g.name, 1, &qh, ());
                estado.novo_seat(&seat, &qh);
            }
        }
        fila.roundtrip(&mut estado).map_err(io::Error::other)?;
        Ok((fila, estado))
    }

    fn rodar(mut fila: EventQueue<Estado>, mut estado: Estado, parar: &AtomicBool) {
        let espera = Timespec {
            tv_sec: 0,
            tv_nsec: 200_000_000,
        };
        while !parar.load(Ordering::Relaxed) {
            if fila.dispatch_pending(&mut estado).is_err() || fila.flush().is_err() {
                break;
            }
            let Some(leitura) = fila.prepare_read() else {
                continue;
            };
            let pronto = {
                let fd = leitura.connection_fd();
                let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                matches!(poll(&mut fds, Some(&espera)), Ok(n) if n > 0)
            };
            if pronto {
                match leitura.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
            }
        }
        estado.soltar_tudo();
        let _ = fila.flush();
    }

    impl Estado {
        fn novo_seat(&mut self, seat: &WlSeat, qh: &QueueHandle<Estado>) {
            self.dispositivos
                .push(self.gerente.get_data_device(seat, qh, ()));
        }

        fn soltar_tudo(&mut self) {
            if let Some(o) = self.atual.take() {
                o.destroy();
            }
            for d in self.dispositivos.drain(..) {
                if d.version() >= 2 {
                    d.release();
                }
            }
        }

        fn drop_atual(&mut self) {
            let Some(oferta) = self.atual.take() else {
                return;
            };
            let caminhos = match receber(&self.conexao, &oferta) {
                Ok(t) => caminhos_de_uri_list(&t),
                Err(_) => Vec::new(),
            };
            if oferta.version() >= 3 {
                oferta.finish();
            }
            oferta.destroy();
            (self.aviso)(Soltura::Saiu);
            if !caminhos.is_empty() {
                (self.aviso)(Soltura::Soltou(caminhos));
            }
        }
    }

    /// Lê o `text/uri-list` da oferta por um pipe (o app de origem escreve e fecha).
    fn receber(conexao: &Connection, oferta: &WlDataOffer) -> io::Result<String> {
        let (leitura, escrita) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
        oferta.receive(URI_LIST.into(), escrita.as_fd());
        conexao.flush().map_err(io::Error::other)?;
        drop(escrita);
        rustix::fs::fcntl_setfl(&leitura, rustix::fs::OFlags::NONBLOCK)?;
        let mut arquivo = std::fs::File::from(leitura);
        let limite = Instant::now() + LEITURA_MAX;
        let mut bytes = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match arquivo.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.extend_from_slice(&buf[..n]);
                    // Uma lista de URIs não passa disso; corta quem mandar lixo sem fim.
                    if bytes.len() > 4 << 20 {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let falta = limite.saturating_duration_since(Instant::now());
                    if falta.is_zero() {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    let espera = Timespec {
                        tv_sec: falta.as_secs() as i64,
                        tv_nsec: i64::from(falta.subsec_nanos()),
                    };
                    let mut fds = [PollFd::new(&arquivo, PollFlags::IN)];
                    let _ = poll(&mut fds, Some(&espera));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        String::from_utf8(bytes).map_err(|_| io::ErrorKind::InvalidData.into())
    }

    impl Dispatch<WlRegistry, GlobalListContents> for Estado {
        fn event(
            estado: &mut Self,
            registry: &WlRegistry,
            event: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            // Um seat que aparece depois (outro teclado/mouse) também recebe arrastos.
            if let wl_registry::Event::Global {
                name, interface, ..
            } = event
                && interface == WlSeat::interface().name
            {
                let seat: WlSeat = registry.bind(name, 1, qh, ());
                estado.novo_seat(&seat, qh);
            }
        }
    }

    impl Dispatch<WlSeat, ()> for Estado {
        fn event(
            _: &mut Self,
            _: &WlSeat,
            _: <WlSeat as Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<WlDataDeviceManager, ()> for Estado {
        fn event(
            _: &mut Self,
            _: &WlDataDeviceManager,
            _: <WlDataDeviceManager as Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<WlDataDevice, ()> for Estado {
        fn event(
            estado: &mut Self,
            _: &WlDataDevice,
            event: wl_data_device::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            match event {
                wl_data_device::Event::Enter {
                    serial,
                    surface,
                    id,
                    ..
                } => {
                    if let Some(o) = estado.atual.take() {
                        o.destroy();
                    }
                    let Some(oferta) = id else {
                        return;
                    };
                    let arquivos = oferta.data::<Tipos>().is_some_and(|t| t.tem(URI_LIST));
                    if surface.id() != estado.superficie || !arquivos {
                        // Outra janela do app, ou não são arquivos: recusa.
                        oferta.accept(serial, None);
                        oferta.destroy();
                        return;
                    }
                    oferta.accept(serial, Some(URI_LIST.into()));
                    if oferta.version() >= 3 {
                        oferta.set_actions(DndAction::Copy, DndAction::Copy);
                    }
                    estado.atual = Some(oferta);
                    (estado.aviso)(Soltura::Entrou);
                }
                wl_data_device::Event::Leave => {
                    if let Some(o) = estado.atual.take() {
                        o.destroy();
                        (estado.aviso)(Soltura::Saiu);
                    }
                }
                wl_data_device::Event::Drop => estado.drop_atual(),
                // Ofertas da área de transferência não são deste módulo.
                wl_data_device::Event::Selection { id: Some(o) } => o.destroy(),
                _ => {}
            }
        }

        event_created_child!(Estado, WlDataDevice, [
            wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Tipos::default()),
        ]);
    }

    impl Dispatch<WlDataOffer, Tipos> for Estado {
        fn event(
            _: &mut Self,
            _: &WlDataOffer,
            event: wl_data_offer::Event,
            tipos: &Tipos,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_data_offer::Event::Offer { mime_type } = event
                && let Ok(mut v) = tipos.0.lock()
                && v.len() < 64
            {
                v.push(mime_type);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::caminhos_de_uri_list;
    use std::path::PathBuf;

    #[test]
    fn uri_list() {
        let t =
            "# comentário\r\nfile:///home/y/a%20b.txt\r\n\r\nhttps://x/y\r\nfile:///tmp/pasta\n";
        assert_eq!(
            caminhos_de_uri_list(t),
            vec![
                PathBuf::from("/home/y/a b.txt"),
                PathBuf::from("/tmp/pasta")
            ]
        );
        assert!(caminhos_de_uri_list("").is_empty());
    }
}
