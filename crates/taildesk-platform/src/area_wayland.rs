//! Área de transferência no Wayland pela janela da sessão do controlador.
//!
//! Uma conexão "convidada" no `wl_display` da janela (fila própria, numa thread, como em
//! [`crate::soltar`]) usa o `wl_data_device` do protocolo base: a seleção chega quando a janela
//! ganha o foco do teclado (é quando a pessoa volta para a sessão depois de copiar em outro
//! app), e a nossa oferta entra com o serial do último evento de teclado da janela. Nada de
//! protocolo de gerenciador de área de transferência: só o que qualquer app com foco faz.

use std::ffi::c_void;
use std::io::{self, Read};
use std::os::fd::{AsFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_client::backend::{Backend, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child,
};

/// O que a área de transferência avisa (na thread dela: só repasse).
#[derive(Debug)]
pub enum DaArea {
    /// Alguém copiou em outro app; os tipos MIME oferecidos.
    Copiou(Vec<String>),
    /// Um app colou a nossa oferta: escreva o conteúdo de `mime` em `fd` e feche (de outra
    /// thread; pode demorar o quanto o peer demorar).
    Colar { mime: String, fd: OwnedFd },
}

enum Comando {
    Oferecer(Vec<String>),
    Ler {
        mime: String,
        resposta: mpsc::Sender<io::Result<OwnedFd>>,
    },
}

/// A área de transferência da janela; soltar para a thread e larga a oferta.
#[derive(Debug)]
pub struct AreaWayland {
    comandos: mpsc::Sender<Comando>,
    /// Escrever um byte acorda a thread para ler os comandos.
    acordar: OwnedFd,
    parar: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Comando {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Comando::Oferecer(m) => f.debug_tuple("Oferecer").field(m).finish(),
            Comando::Ler { mime, .. } => f.debug_struct("Ler").field("mime", mime).finish(),
        }
    }
}

impl Drop for AreaWayland {
    fn drop(&mut self) {
        self.parar.store(true, Ordering::Relaxed);
        let _ = rustix::io::write(&self.acordar, &[1]);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Ponteiro da libwayland que atravessa para a thread.
struct Ptr(NonNull<c_void>);
// SAFETY: a libwayland-client é thread-safe para conexões com filas separadas; o ponteiro só
// cria a conexão convidada.
unsafe impl Send for Ptr {}

/// Quanto esperar o app de origem escrever o conteúdo.
const LEITURA_MAX: Duration = Duration::from_secs(5);

impl AreaWayland {
    /// Começa a acompanhar a área de transferência. `aviso` roda na thread da área.
    ///
    /// # Safety
    ///
    /// `display` tem de ser o `wl_display` vivo da janela (do `raw-window-handle`), e tem de
    /// viver mais que a [`AreaWayland`].
    pub unsafe fn abrir(
        display: NonNull<c_void>,
        aviso: impl FnMut(DaArea) + Send + 'static,
    ) -> io::Result<AreaWayland> {
        let (comandos, rx) = mpsc::channel();
        let (acordado, acordar) = rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )?;
        let parar = Arc::new(AtomicBool::new(false));
        let (pronto_tx, pronto_rx) = mpsc::sync_channel(1);
        let d = Ptr(display);
        let p = parar.clone();
        let thread = std::thread::Builder::new()
            .name("taildesk-area".into())
            .spawn(move || {
                let d = d;
                // SAFETY: repassado do contrato de `abrir`.
                match unsafe { iniciar(d.0, Box::new(aviso)) } {
                    Ok((fila, estado)) => {
                        let _ = pronto_tx.send(Ok(()));
                        rodar(fila, estado, &rx, &acordado, &p);
                    }
                    Err(e) => {
                        let _ = pronto_tx.send(Err(e));
                    }
                }
            })?;
        pronto_rx
            .recv()
            .map_err(|_| io::Error::other("área de transferência terminou"))??;
        Ok(AreaWayland {
            comandos,
            acordar,
            parar,
            thread: Some(thread),
        })
    }

    fn mandar(&self, c: Comando) -> io::Result<()> {
        self.comandos
            .send(c)
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        let _ = rustix::io::write(&self.acordar, &[1]);
        Ok(())
    }

    /// Põe uma oferta nossa com estes tipos (o conteúdo vem por [`DaArea::Colar`]). Sem foco
    /// na janela, entra quando ela ganhar o foco.
    pub fn oferecer(&self, mimes: Vec<String>) -> io::Result<()> {
        self.mandar(Comando::Oferecer(mimes))
    }

    /// Lê `mime` da seleção atual (bloqueia até o app de origem terminar ou [`LEITURA_MAX`]).
    pub fn ler(&self, mime: &str, limite: usize) -> io::Result<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        self.mandar(Comando::Ler {
            mime: mime.to_owned(),
            resposta: tx,
        })?;
        let fd = rx
            .recv_timeout(LEITURA_MAX)
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        ler_tudo(fd, limite, LEITURA_MAX)
    }
}

/// Lê um pipe até o fim, com limite de bytes e de tempo.
pub fn ler_tudo(fd: OwnedFd, limite: usize, prazo: Duration) -> io::Result<Vec<u8>> {
    rustix::fs::fcntl_setfl(&fd, rustix::fs::OFlags::NONBLOCK)?;
    let mut arquivo = std::fs::File::from(fd);
    let fim = Instant::now() + prazo;
    let mut bytes = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match arquivo.read(&mut buf) {
            Ok(0) => return Ok(bytes),
            Ok(n) => {
                bytes.extend_from_slice(&buf[..n]);
                if bytes.len() > limite {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "conteúdo grande demais",
                    ));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let falta = fim.saturating_duration_since(Instant::now());
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
}

/// Tipos MIME anunciados por uma oferta.
#[derive(Default)]
struct Tipos(Mutex<Vec<String>>);

impl Tipos {
    fn lista(&self) -> Vec<String> {
        self.0.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

struct Estado {
    conexao: Connection,
    gerente: WlDataDeviceManager,
    seats: Vec<(WlSeat, Option<WlDataDevice>, Option<WlKeyboard>)>,
    /// Seleção atual (oferta de outro app, ou eco da nossa).
    selecao: Option<WlDataOffer>,
    /// Nossa oferta enquanto é a seleção (ou espera o foco para entrar).
    fonte: Option<WlDataSource>,
    pendente: bool,
    /// Serial do último evento de teclado com foco numa janela nossa.
    serial: Option<u32>,
    aviso: Box<dyn FnMut(DaArea) + Send>,
}

unsafe fn iniciar(
    display: NonNull<c_void>,
    aviso: Box<dyn FnMut(DaArea) + Send>,
) -> io::Result<(EventQueue<Estado>, Estado)> {
    // SAFETY: `display` é o `wl_display` vivo da janela (contrato de `AreaWayland::abrir`).
    let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
    let conexao = Connection::from_backend(backend);
    let (globais, mut fila) = registry_queue_init::<Estado>(&conexao).map_err(io::Error::other)?;
    let qh = fila.handle();
    let gerente: WlDataDeviceManager = globais
        .bind(&qh, 1..=3, ())
        .map_err(|_| io::Error::from(io::ErrorKind::Unsupported))?;
    let mut estado = Estado {
        conexao,
        gerente,
        seats: Vec::new(),
        selecao: None,
        fonte: None,
        pendente: false,
        serial: None,
        aviso,
    };
    for g in globais.contents().clone_list() {
        if g.interface == WlSeat::interface().name {
            let seat: WlSeat = globais.registry().bind(g.name, 1, &qh, ());
            estado.novo_seat(seat, &qh);
        }
    }
    fila.roundtrip(&mut estado).map_err(io::Error::other)?;
    Ok((fila, estado))
}

fn rodar(
    mut fila: EventQueue<Estado>,
    mut estado: Estado,
    comandos: &mpsc::Receiver<Comando>,
    acordado: &OwnedFd,
    parar: &AtomicBool,
) {
    let qh = fila.handle();
    let espera = Timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };
    while !parar.load(Ordering::Relaxed) {
        while let Ok(c) = comandos.try_recv() {
            estado.comando(c, &qh);
        }
        if fila.dispatch_pending(&mut estado).is_err() || fila.flush().is_err() {
            break;
        }
        let Some(leitura) = fila.prepare_read() else {
            continue;
        };
        let (wayland, acordou) = {
            let fd = leitura.connection_fd();
            let mut fds = [
                PollFd::new(&fd, PollFlags::IN),
                PollFd::new(acordado, PollFlags::IN),
            ];
            let n = poll(&mut fds, Some(&espera)).unwrap_or(0);
            (
                n > 0 && fds[0].revents().contains(PollFlags::IN),
                n > 0 && fds[1].revents().contains(PollFlags::IN),
            )
        };
        if acordou {
            let mut lixo = [0u8; 64];
            while matches!(rustix::io::read(acordado, &mut lixo), Ok(n) if n > 0) {}
        }
        if wayland {
            match leitura.read() {
                Ok(_) => {}
                Err(WaylandError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        } else {
            drop(leitura);
        }
    }
    estado.soltar_tudo();
    let _ = fila.flush();
}

impl Estado {
    fn novo_seat(&mut self, seat: WlSeat, qh: &QueueHandle<Estado>) {
        let dispositivo = self.gerente.get_data_device(&seat, qh, ());
        self.seats.push((seat, Some(dispositivo), None));
    }

    fn comando(&mut self, c: Comando, qh: &QueueHandle<Estado>) {
        match c {
            Comando::Oferecer(mimes) => {
                if let Some(f) = self.fonte.take() {
                    f.destroy();
                }
                if mimes.is_empty() {
                    return;
                }
                let fonte = self.gerente.create_data_source(qh, ());
                for m in mimes {
                    fonte.offer(m);
                }
                self.fonte = Some(fonte);
                self.pendente = true;
                self.aplicar_oferta();
            }
            Comando::Ler { mime, resposta } => {
                let r = match &self.selecao {
                    Some(o) if o.data::<Tipos>().is_some_and(|t| t.lista().contains(&mime)) => {
                        receber(&self.conexao, o, &mime)
                    }
                    _ => Err(io::Error::from(io::ErrorKind::NotFound)),
                };
                let _ = resposta.send(r);
            }
        }
    }

    /// Põe a nossa oferta como seleção se já há um serial de teclado.
    fn aplicar_oferta(&mut self) {
        let (Some(fonte), Some(serial), true) = (&self.fonte, self.serial, self.pendente) else {
            return;
        };
        for (_, d, _) in &self.seats {
            if let Some(d) = d {
                d.set_selection(Some(fonte), serial);
            }
        }
        self.pendente = false;
    }

    fn soltar_tudo(&mut self) {
        if let Some(o) = self.selecao.take() {
            o.destroy();
        }
        if let Some(f) = self.fonte.take() {
            f.destroy();
        }
        for (_, d, k) in self.seats.drain(..) {
            if let Some(d) = d
                && d.version() >= 2
            {
                d.release();
            }
            if let Some(k) = k
                && k.version() >= 3
            {
                k.release();
            }
        }
    }
}

/// Pede à oferta o conteúdo de `mime` por um pipe; devolve a ponta de leitura.
fn receber(conexao: &Connection, oferta: &WlDataOffer, mime: &str) -> io::Result<OwnedFd> {
    let (leitura, escrita) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
    oferta.receive(mime.into(), escrita.as_fd());
    conexao.flush().map_err(io::Error::other)?;
    drop(escrita);
    Ok(leitura)
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
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
            && interface == WlSeat::interface().name
        {
            let seat: WlSeat = registry.bind(name, 1, qh, ());
            estado.novo_seat(seat, qh);
        }
    }
}

impl Dispatch<WlSeat, ()> for Estado {
    fn event(
        estado: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // O teclado é só para o serial que o `set_selection` exige.
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(c),
        } = event
            && let Some((_, _, k)) = estado.seats.iter_mut().find(|(s, _, _)| s == seat)
        {
            let tem = c.contains(wl_seat::Capability::Keyboard);
            if tem && k.is_none() {
                *k = Some(seat.get_keyboard(qh, ()));
            } else if !tem
                && let Some(t) = k.take()
                && t.version() >= 3
            {
                t.release();
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for Estado {
    fn event(
        estado: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { serial, .. } => {
                estado.serial = Some(serial);
                estado.aplicar_oferta();
            }
            wl_keyboard::Event::Key { serial, .. }
            | wl_keyboard::Event::Modifiers { serial, .. } => {
                estado.serial = Some(serial);
            }
            wl_keyboard::Event::Leave { .. } => estado.serial = None,
            // O `Keymap` traz um fd, que se fecha ao cair aqui.
            _ => {}
        }
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
            wl_data_device::Event::Selection { id } => {
                if let Some(o) = estado.selecao.take() {
                    o.destroy();
                }
                let Some(oferta) = id else { return };
                let tipos = oferta.data::<Tipos>().map(Tipos::lista).unwrap_or_default();
                estado.selecao = Some(oferta);
                // Enquanto a nossa fonte não foi cancelada, a seleção é o eco da nossa oferta.
                if estado.fonte.is_none() && !tipos.is_empty() {
                    (estado.aviso)(DaArea::Copiou(tipos));
                }
            }
            // Arrastar é de `crate::soltar`.
            wl_data_device::Event::Enter { id: Some(o), .. } => o.destroy(),
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

impl Dispatch<WlDataSource, ()> for Estado {
    fn event(
        estado: &mut Self,
        fonte: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                (estado.aviso)(DaArea::Colar {
                    mime: mime_type,
                    fd,
                });
            }
            wl_data_source::Event::Cancelled => {
                // Outro app copiou (ou trocamos de oferta).
                if estado.fonte.as_ref() == Some(fonte) {
                    estado.fonte = None;
                    estado.pendente = false;
                }
                fonte.destroy();
            }
            _ => {}
        }
    }
}
