//! Sessão de arquivos: trata as mensagens 80–89 do stream de controle, abre e aceita os
//! streams de dados e aplica as regras do host (pastas liberadas, pasta de entrada, até
//! [`limits::PARALLEL`] envios ao mesmo tempo, auditoria).
//!
//! É igual nos dois lados da sessão: o controlador envia e baixa, o host recebe e serve, e o
//! que cada lado aceita do peer vem do [`Config`] (sem a permissão `files`, use
//! [`Config::closed`]). Quem usa:
//! - repassa cada `Msg` de arquivos recebido para [`Files::handle`];
//! - repassa cada stream unidirecional de arquivos recebido para [`Files::accept_data`];
//! - escreve no stream de controle as [`Control`] que chegam pelo canal `out`;
//! - mostra os [`Event`]s (progresso, fim, falha, listagens).

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use taildesk_proto::control::{
    Control, FileAccept, FileCancel, FileDone, FileError, FileErrorCode, FileOffer, FileRequest,
    FsAccess, FsEntry, FsList, FsListing, FsRoot, FsRoots, FsRootsRequest, Msg,
};
use taildesk_proto::files::{Hash, INBOX_ROOT, limits};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::task::AbortHandle;

use crate::audit::{AuditLog, Direction, Record};
use crate::listing::{list_dir, page};
use crate::resolve_in_root;
use crate::xfer::{Incoming, Source, XferError, read_header};

/// Abre streams de dados para o peer (no app, `quinn::Connection::open_uni`).
pub trait Link: Send + Sync + 'static {
    type Writer: AsyncWrite + Unpin + Send + 'static;
    fn open_data(&self) -> impl Future<Output = io::Result<Self::Writer>> + Send;
}

impl Link for quinn::Connection {
    type Writer = quinn::SendStream;

    async fn open_data(&self) -> io::Result<quinn::SendStream> {
        self.open_uni().await.map_err(io::Error::other)
    }
}

/// Lado da sessão. Separa os `xfer_id`/`req_id` de cada lado: controlador ímpar, host par.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Controller,
    Host,
}

/// Uma pasta liberada para o peer navegar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedRoot {
    /// Diferente de [`INBOX_ROOT`].
    pub id: u32,
    pub name: String,
    pub path: PathBuf,
    pub access: FsAccess,
}

/// O que este lado aceita do peer.
#[derive(Clone, Debug)]
pub struct Config {
    pub side: Side,
    /// Identidade do peer vinda de `Overlay::identify()`, para a auditoria.
    pub peer: String,
    /// Pasta de entrada (`~/Downloads/TailDesk`); `None` = o peer não pode enviar para cá.
    pub inbox: Option<PathBuf>,
    /// Pastas para o navegador remoto.
    pub roots: Vec<SharedRoot>,
    pub audit: Option<AuditLog>,
}

impl Config {
    /// Sem a permissão `files`: o peer não lista, não baixa e não envia nada.
    pub fn closed(side: Side, peer: impl Into<String>) -> Config {
        Config {
            side,
            peer: peer.into(),
            inbox: None,
            roots: Vec::new(),
            audit: None,
        }
    }
}

/// O que a interface mostra.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// O peer começou a mandar um arquivo (aceito).
    Receiving {
        xfer_id: u32,
        name: String,
        size: u64,
    },
    /// Progresso; pode pular valores (canal "latest-wins").
    Progress {
        xfer_id: u32,
        done: u64,
        total: u64,
    },
    Finished {
        xfer_id: u32,
        direction: Direction,
        path: PathBuf,
    },
    /// Falha ou recusa. Com `resumable`, o `.part` ficou e uma nova oferta retoma.
    Failed {
        xfer_id: u32,
        direction: Direction,
        code: FileErrorCode,
        resumable: bool,
    },
    Roots(FsRoots),
    Listing(FsListing),
}

/// Oferta feita, esperando o `FileAccept`.
#[derive(Debug)]
struct Outgoing {
    source: Source,
    path: PathBuf,
    _permit: Option<OwnedSemaphorePermit>,
}

/// Oferta aceita, esperando o stream de dados e o `FileDone`.
#[derive(Debug)]
struct Receiving {
    incoming: Option<Incoming>,
    done_tx: Option<oneshot::Sender<Hash>>,
    done_rx: Option<oneshot::Receiver<Hash>>,
}

#[derive(Debug, Default)]
struct State {
    outgoing: HashMap<u32, Outgoing>,
    incoming: HashMap<u32, Receiving>,
    /// Pedidos de download feitos por este lado: `xfer_id` → pasta de destino.
    downloads: HashMap<u32, PathBuf>,
    tasks: HashMap<u32, (AbortHandle, Direction)>,
}

#[derive(Debug)]
struct Inner<L> {
    cfg: Config,
    link: L,
    out: mpsc::Sender<Control>,
    events: mpsc::Sender<Event>,
    next_id: AtomicU32,
    send_slots: Arc<Semaphore>,
    state: Mutex<State>,
}

/// Sessão de arquivos de uma conexão. Barata de clonar.
#[derive(Debug)]
pub struct Files<L> {
    inner: Arc<Inner<L>>,
}

impl<L> Clone for Files<L> {
    fn clone(&self) -> Self {
        Files {
            inner: self.inner.clone(),
        }
    }
}

/// Recebidas pendentes além disto são recusadas com `BUSY` (proteção contra ofertas em massa).
const MAX_PENDING_IN: usize = 4 * limits::PARALLEL;

/// Progresso é avisado a cada tanto de bytes (e no fim).
const PROGRESS_STEP: u64 = 1024 * 1024;

/// Nome do código para a auditoria.
fn code_name(code: FileErrorCode) -> &'static str {
    match code {
        FileErrorCode::Unspecified => "UNSPECIFIED",
        FileErrorCode::NotFound => "NOT_FOUND",
        FileErrorCode::Denied => "DENIED",
        FileErrorCode::BadPath => "BAD_PATH",
        FileErrorCode::NoSpace => "NO_SPACE",
        FileErrorCode::HashMismatch => "HASH_MISMATCH",
        FileErrorCode::ResumeMismatch => "RESUME_MISMATCH",
        FileErrorCode::Io => "IO",
        FileErrorCode::Busy => "BUSY",
        FileErrorCode::Cancelled => "CANCELLED",
        FileErrorCode::Changed => "CHANGED",
        FileErrorCode::Protocol => "PROTOCOL",
    }
}

impl<L: Link> Files<L> {
    pub fn new(
        cfg: Config,
        link: L,
        out: mpsc::Sender<Control>,
        events: mpsc::Sender<Event>,
    ) -> Files<L> {
        let first = match cfg.side {
            Side::Controller => 1,
            Side::Host => 2,
        };
        Files {
            inner: Arc::new(Inner {
                cfg,
                link,
                out,
                events,
                next_id: AtomicU32::new(first),
                send_slots: Arc::new(Semaphore::new(limits::PARALLEL)),
                state: Mutex::new(State::default()),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // Nenhuma seção crítica entra em pânico; um lock envenenado ainda tem dados coerentes.
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn new_id(&self) -> u32 {
        self.inner.next_id.fetch_add(2, Ordering::Relaxed)
    }

    async fn send(&self, msg: Msg) {
        // Canal fechado = sessão acabando; não há a quem avisar.
        let _ = self.inner.out.send(msg.into()).await;
    }

    async fn emit(&self, e: Event) {
        let _ = self.inner.events.send(e).await;
    }

    fn progress(&self, xfer_id: u32, total: u64) -> impl FnMut(u64) + Send + 'static {
        let events = self.inner.events.clone();
        let mut last = 0;
        move |done| {
            if done == total || done >= last + PROGRESS_STEP {
                last = done;
                let _ = events.try_send(Event::Progress {
                    xfer_id,
                    done,
                    total,
                });
            }
        }
    }

    fn audit(
        &self,
        direction: Direction,
        path: &Path,
        size: u64,
        blake3: Option<Hash>,
        result: &str,
    ) {
        if let Some(log) = &self.inner.cfg.audit {
            let rec = Record {
                peer: &self.inner.cfg.peer,
                direction,
                path,
                size,
                blake3,
                result,
            };
            // A transferência já aconteceu; falha no registro não a desfaz.
            let _ = log.append(&rec);
        }
    }

    async fn reject(&self, xfer_id: u32, code: FileErrorCode) {
        self.send(Msg::FileError(FileError {
            xfer_id,
            code: code.into(),
        }))
        .await;
    }

    fn root(&self, id: u32) -> Option<&SharedRoot> {
        self.inner.cfg.roots.iter().find(|r| r.id == id)
    }

    fn readable_root(&self, id: u32) -> Result<&Path, FileErrorCode> {
        match self.root(id) {
            Some(r) if matches!(r.access, FsAccess::Read | FsAccess::ReadWrite) => Ok(&r.path),
            _ => Err(FileErrorCode::Denied),
        }
    }

    fn writable_root(&self, id: u32) -> Result<&Path, FileErrorCode> {
        if id == INBOX_ROOT {
            return self.inner.cfg.inbox.as_deref().ok_or(FileErrorCode::Denied);
        }
        match self.root(id) {
            Some(r) if matches!(r.access, FsAccess::Write | FsAccess::ReadWrite) => Ok(&r.path),
            _ => Err(FileErrorCode::Denied),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Ações locais

    /// Pede as pastas que o peer libera; a resposta chega como [`Event::Roots`].
    pub async fn request_roots(&self) {
        self.send(Msg::FsRootsRequest(FsRootsRequest {})).await;
    }

    /// Pede uma página da pasta `path` de `root`; a resposta chega como [`Event::Listing`] com
    /// o `req_id` devolvido.
    pub async fn list(&self, root: u32, path: &str, start: u32) -> u32 {
        let req_id = self.new_id();
        self.send(Msg::FsList(FsList {
            req_id,
            root,
            path: path.to_owned(),
            start,
        }))
        .await;
        req_id
    }

    /// Oferece o arquivo local `path` para `rel_path` dentro de `root` do peer
    /// ([`INBOX_ROOT`] = pasta de entrada). Espera uma vaga se já houver
    /// [`limits::PARALLEL`] envios em andamento.
    pub async fn send_file(
        &self,
        path: &Path,
        root: u32,
        rel_path: &str,
    ) -> Result<u32, XferError> {
        let permit = self
            .inner
            .send_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| XferError::Io(io::ErrorKind::BrokenPipe.into()))?;
        let source = Source::open(path).await?;
        let xfer_id = self.new_id();
        let offer = FileOffer {
            xfer_id,
            root,
            rel_path: rel_path.to_owned(),
            size: source.size(),
            mtime_ms: source.mtime_ms(),
        };
        self.state().outgoing.insert(
            xfer_id,
            Outgoing {
                source,
                path: path.to_owned(),
                _permit: Some(permit),
            },
        );
        self.send(Msg::FileOffer(offer)).await;
        Ok(xfer_id)
    }

    /// Oferece uma pasta inteira (sem seguir links), arquivo por arquivo.
    pub async fn send_folder(&self, dir: &Path, root: u32) -> Result<Vec<u32>, XferError> {
        let dir2 = dir.to_owned();
        let walk = tokio::task::spawn_blocking(move || crate::listing::walk(&dir2))
            .await
            .map_err(|e| XferError::Io(io::Error::other(e)))??;
        let mut ids = Vec::with_capacity(walk.files.len());
        for f in walk.files {
            ids.push(self.send_file(&f.path, root, &f.rel_path).await?);
        }
        Ok(ids)
    }

    /// Baixa `path` de `root` do peer para a pasta local `dest_dir`.
    pub async fn download(&self, root: u32, path: &str, dest_dir: &Path) -> u32 {
        let xfer_id = self.new_id();
        self.state().downloads.insert(xfer_id, dest_dir.to_owned());
        self.send(Msg::FileRequest(FileRequest {
            xfer_id,
            root,
            path: path.to_owned(),
        }))
        .await;
        xfer_id
    }

    /// Cancela uma transferência nos dois lados. O `.part` fica para retomar.
    pub async fn cancel(&self, xfer_id: u32) {
        if self.forget(xfer_id) {
            self.send(Msg::FileCancel(FileCancel { xfer_id })).await;
        }
    }

    fn forget(&self, xfer_id: u32) -> bool {
        let mut st = self.state();
        let t = st.tasks.remove(&xfer_id).map(|(t, _)| t.abort()).is_some();
        let a = st.outgoing.remove(&xfer_id).is_some();
        let b = st.incoming.remove(&xfer_id).is_some();
        let c = st.downloads.remove(&xfer_id).is_some();
        t || a || b || c
    }

    // -----------------------------------------------------------------------------------------
    // Mensagens do peer

    /// Trata uma mensagem de arquivos do stream de controle. Outras mensagens são ignoradas.
    pub async fn handle(&self, msg: Msg) {
        match msg {
            Msg::FsRootsRequest(_) => self.on_roots_request().await,
            Msg::FsRoots(m) => self.emit(Event::Roots(m)).await,
            Msg::FsList(m) => self.on_list(m).await,
            Msg::FsListing(m) => self.emit(Event::Listing(m)).await,
            Msg::FileOffer(m) => self.on_offer(m).await,
            Msg::FileRequest(m) => self.on_request(m).await,
            Msg::FileAccept(m) => self.on_accept(m).await,
            Msg::FileCancel(m) => self.on_cancel(m.xfer_id).await,
            Msg::FileDone(m) => self.on_done(m),
            Msg::FileError(m) => self.on_error(m).await,
            _ => {}
        }
    }

    async fn on_roots_request(&self) {
        let roots = self
            .inner
            .cfg
            .roots
            .iter()
            .map(|r| FsRoot {
                id: r.id,
                name: r.name.clone(),
                access: r.access.into(),
            })
            .collect();
        self.send(Msg::FsRoots(FsRoots { roots })).await;
    }

    async fn on_list(&self, m: FsList) {
        let reply = match self.readable_root(m.root) {
            Err(code) => listing_error(m.req_id, code),
            Ok(root) => {
                let root = root.to_owned();
                let path = m.path.clone();
                let listed = tokio::task::spawn_blocking(move || list_dir(&root, &path)).await;
                match listed {
                    Ok(Ok(all)) => {
                        let (entries, more) = page(&all, m.start);
                        FsListing {
                            req_id: m.req_id,
                            entries: entries.iter().map(FsEntry::from).collect(),
                            more,
                            error: FileErrorCode::Unspecified.into(),
                        }
                    }
                    Ok(Err(e)) => listing_error(m.req_id, XferError::Path(e).code()),
                    Err(_) => listing_error(m.req_id, FileErrorCode::Io),
                }
            }
        };
        self.send(Msg::FsListing(reply)).await;
    }

    async fn on_request(&self, m: FileRequest) {
        let path = match self.readable_root(m.root) {
            Err(code) => return self.reject(m.xfer_id, code).await,
            Ok(root) => match resolve_in_root(root, &m.path) {
                Ok(p) => p,
                Err(e) => return self.reject(m.xfer_id, XferError::Path(e).code()).await,
            },
        };
        let source = match Source::open(&path).await {
            Ok(s) => s,
            Err(e) => return self.reject(m.xfer_id, e.code()).await,
        };
        let name = m.path.rsplit('/').next().unwrap_or_default().to_owned();
        let offer = FileOffer {
            xfer_id: m.xfer_id,
            root: INBOX_ROOT,
            rel_path: name,
            size: source.size(),
            mtime_ms: source.mtime_ms(),
        };
        // Downloads pedidos pelo peer não usam as vagas locais: quem pede controla o ritmo.
        let taken = {
            let mut st = self.state();
            let taken = st.outgoing.contains_key(&m.xfer_id);
            if !taken {
                st.outgoing.insert(
                    m.xfer_id,
                    Outgoing {
                        source,
                        path,
                        _permit: None,
                    },
                );
            }
            taken
        };
        if taken {
            return self.reject(m.xfer_id, FileErrorCode::Busy).await;
        }
        self.send(Msg::FileOffer(offer)).await;
    }

    async fn on_offer(&self, m: FileOffer) {
        // Resposta a um download nosso: vai para a pasta escolhida aqui, com o nome do arquivo.
        let download = self.state().downloads.remove(&m.xfer_id);
        let (root, rel) = match &download {
            Some(dir) => {
                let name = m.rel_path.rsplit('/').next().unwrap_or_default();
                (dir.as_path(), name.to_owned())
            }
            None => match self.writable_root(m.root) {
                Ok(root) => (root, m.rel_path.clone()),
                Err(code) => return self.reject(m.xfer_id, code).await,
            },
        };
        let busy = {
            let st = self.state();
            st.incoming.contains_key(&m.xfer_id) || st.incoming.len() >= MAX_PENDING_IN
        };
        if busy {
            return self.reject(m.xfer_id, FileErrorCode::Busy).await;
        }
        let incoming = match Incoming::prepare(root, &rel, m.size, m.mtime_ms).await {
            Ok(i) => i,
            Err(e) => {
                let code = e.code();
                self.audit(
                    Direction::In,
                    Path::new(&rel),
                    m.size,
                    None,
                    code_name(code),
                );
                return self.reject(m.xfer_id, code).await;
            }
        };
        let accept = FileAccept {
            xfer_id: m.xfer_id,
            offset: incoming.offset(),
            prefix_blake3: incoming.prefix().map(Vec::from).unwrap_or_default(),
        };
        let (done_tx, done_rx) = oneshot::channel();
        self.state().incoming.insert(
            m.xfer_id,
            Receiving {
                incoming: Some(incoming),
                done_tx: Some(done_tx),
                done_rx: Some(done_rx),
            },
        );
        self.emit(Event::Receiving {
            xfer_id: m.xfer_id,
            name: rel,
            size: m.size,
        })
        .await;
        self.send(Msg::FileAccept(accept)).await;
    }

    async fn on_accept(&self, m: FileAccept) {
        let Some(mut out) = self.state().outgoing.remove(&m.xfer_id) else {
            return;
        };
        let prefix: Option<Hash> = match m.prefix_blake3.len() {
            0 => None,
            _ => match m.prefix_blake3.as_slice().try_into() {
                Ok(h) => Some(h),
                Err(_) => return self.reject(m.xfer_id, FileErrorCode::Protocol).await,
            },
        };
        let me = self.clone();
        let xfer_id = m.xfer_id;
        let task = tokio::spawn(async move {
            let total = out.source.size();
            let result = async {
                let mut w = me.inner.link.open_data().await?;
                let mut progress = me.progress(xfer_id, total);
                let hash = out
                    .source
                    .send(xfer_id, m.offset, prefix, &mut w, &mut progress)
                    .await?;
                w.shutdown().await?;
                Ok::<_, XferError>(hash)
            }
            .await;
            me.state().tasks.remove(&xfer_id);
            match result {
                Ok(hash) => {
                    me.send(Msg::FileDone(FileDone {
                        xfer_id,
                        blake3: hash.to_vec(),
                    }))
                    .await;
                    me.audit(Direction::Out, &out.path, total, Some(hash), "ok");
                    me.emit(Event::Finished {
                        xfer_id,
                        direction: Direction::Out,
                        path: out.path,
                    })
                    .await;
                }
                Err(XferError::ResumeMismatch) => {
                    // Quem recebe recomeça do zero e manda outro `FileAccept`.
                    me.state().outgoing.insert(xfer_id, out);
                    me.reject(xfer_id, FileErrorCode::ResumeMismatch).await;
                }
                Err(e) => {
                    let code = e.code();
                    me.reject(xfer_id, code).await;
                    me.audit(Direction::Out, &out.path, total, None, code_name(code));
                    me.emit(Event::Failed {
                        xfer_id,
                        direction: Direction::Out,
                        code,
                        resumable: e.resumable(),
                    })
                    .await;
                }
            }
        });
        self.track(xfer_id, Direction::Out, &task);
    }

    /// Guarda a tarefa para `cancel`, a menos que ela já tenha terminado.
    fn track(&self, xfer_id: u32, direction: Direction, task: &tokio::task::JoinHandle<()>) {
        let mut st = self.state();
        if !task.is_finished() {
            st.tasks.insert(xfer_id, (task.abort_handle(), direction));
        }
    }

    fn direction_of(&self, xfer_id: u32) -> Direction {
        let st = self.state();
        match st.tasks.get(&xfer_id) {
            Some((_, d)) => *d,
            None if st.outgoing.contains_key(&xfer_id) => Direction::Out,
            None => Direction::In,
        }
    }

    fn on_done(&self, m: FileDone) {
        let Ok(hash) = Hash::try_from(m.blake3.as_slice()) else {
            return;
        };
        if let Some(tx) = self
            .state()
            .incoming
            .get_mut(&m.xfer_id)
            .and_then(|r| r.done_tx.take())
        {
            let _ = tx.send(hash);
        }
    }

    async fn on_cancel(&self, xfer_id: u32) {
        let direction = self.direction_of(xfer_id);
        if self.forget(xfer_id) {
            self.emit(Event::Failed {
                xfer_id,
                direction,
                code: FileErrorCode::Cancelled,
                resumable: true,
            })
            .await;
        }
    }

    async fn on_error(&self, m: FileError) {
        let code = m.code();
        if code == FileErrorCode::ResumeMismatch {
            let incoming = self
                .state()
                .incoming
                .get_mut(&m.xfer_id)
                .and_then(|r| r.incoming.take());
            if let Some(mut incoming) = incoming {
                let restarted = incoming.restart().await;
                if let Some(r) = self.state().incoming.get_mut(&m.xfer_id) {
                    r.incoming = Some(incoming);
                }
                if restarted.is_ok() {
                    self.send(Msg::FileAccept(FileAccept {
                        xfer_id: m.xfer_id,
                        offset: 0,
                        prefix_blake3: Vec::new(),
                    }))
                    .await;
                    return;
                }
            }
        }
        let direction = self.direction_of(m.xfer_id);
        if self.forget(m.xfer_id) {
            self.emit(Event::Failed {
                xfer_id: m.xfer_id,
                direction,
                code,
                resumable: matches!(code, FileErrorCode::Io | FileErrorCode::Cancelled),
            })
            .await;
        }
    }

    // -----------------------------------------------------------------------------------------
    // Streams de dados do peer

    /// Recebe um stream de dados aberto pelo peer, numa tarefa própria.
    pub fn accept_data<R: AsyncRead + Unpin + Send + 'static>(&self, mut input: R) {
        let me = self.clone();
        tokio::spawn(async move {
            let Ok(header) = read_header(&mut input).await else {
                return;
            };
            let xfer_id = header.xfer_id;
            let taken = me
                .state()
                .incoming
                .get_mut(&xfer_id)
                .and_then(|r| Some((r.incoming.take()?, r.done_rx.take()?)));
            let Some((mut incoming, done_rx)) = taken else {
                // Stream sem oferta aceita: descarta.
                return;
            };
            let me2 = me.clone();
            let task = tokio::spawn(async move {
                let me = me2;
                let total = incoming.size();
                let part = incoming.part_path().to_owned();
                let mut progress = me.progress(xfer_id, total);
                let result = async {
                    incoming
                        .receive_after_header(header, &mut input, &mut progress)
                        .await?;
                    let hash = done_rx.await.map_err(|_| XferError::Incomplete {
                        received: incoming.offset(),
                    })?;
                    let path = incoming.finish(hash).await?;
                    Ok::<_, XferError>((path, hash))
                }
                .await;
                {
                    let mut st = me.state();
                    st.incoming.remove(&xfer_id);
                    st.tasks.remove(&xfer_id);
                }
                match result {
                    Ok((path, hash)) => {
                        me.audit(Direction::In, &path, total, Some(hash), "ok");
                        me.emit(Event::Finished {
                            xfer_id,
                            direction: Direction::In,
                            path,
                        })
                        .await;
                    }
                    Err(e) => {
                        let code = e.code();
                        if !matches!(e, XferError::Incomplete { .. }) {
                            me.reject(xfer_id, code).await;
                        }
                        me.audit(Direction::In, &part, total, None, code_name(code));
                        me.emit(Event::Failed {
                            xfer_id,
                            direction: Direction::In,
                            code,
                            resumable: e.resumable(),
                        })
                        .await;
                    }
                }
            });
            me.track(xfer_id, Direction::In, &task);
        });
    }
}

fn listing_error(req_id: u32, code: FileErrorCode) -> FsListing {
    FsListing {
        req_id,
        entries: Vec::new(),
        more: false,
        error: code.into(),
    }
}
