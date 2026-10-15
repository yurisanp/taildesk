//! Envio e recepção dos bytes de um arquivo pelo stream de dados (`10-plano` §3, "Fio").
//!
//! - Quem envia: [`Source::open`], depois [`Source::send`] com o `offset` e o prefixo do
//!   `Accept`. O BLAKE3 devolvido vai no `Done`.
//! - Quem recebe: [`Incoming::prepare`] (acha o `.part` e calcula o `offset`), manda o `Accept`,
//!   chama [`Incoming::receive`] com o stream e, ao chegar o `Done`, [`Incoming::finish`].
//!
//! Os dois lados calculam o BLAKE3 em streaming, inclusive do prefixo já transferido, então a
//! retomada confere o prefixo antes de mandar um byte e o arquivo inteiro no fim.
//! Os streams são genéricos (`AsyncRead`/`AsyncWrite`): no app são streams uni do `quinn`.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use taildesk_proto::control::FileErrorCode;
use taildesk_proto::files::{DataHeader, Hash, limits};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

use crate::{PathError, part_path, resolve_in_root, unique_path};

/// Falha numa transferência.
#[derive(Debug)]
pub enum XferError {
    Path(PathError),
    Io(io::Error),
    /// O stream de dados não começa com um cabeçalho válido para esta transferência.
    BadHeader,
    /// O stream terminou antes do fim; o `.part` fica para retomar.
    Incomplete {
        received: u64,
    },
    /// O peer mandou mais bytes do que ofereceu.
    TooLong,
    /// O prefixo do `.part` não bate com o arquivo de quem envia.
    ResumeMismatch,
    /// O arquivo recebido não bate com o BLAKE3 do `Done`; o `.part` foi apagado.
    HashMismatch,
    /// O arquivo de origem mudou de tamanho durante o envio.
    Changed,
}

impl XferError {
    /// O `.part` continua válido e uma nova oferta retoma de onde parou.
    pub fn resumable(&self) -> bool {
        matches!(self, XferError::Incomplete { .. } | XferError::Io(_))
    }

    /// Código para o `FileError` que avisa o peer.
    pub fn code(&self) -> FileErrorCode {
        match self {
            XferError::Path(PathError::Io(io::ErrorKind::NotFound)) => FileErrorCode::NotFound,
            XferError::Path(PathError::Io(io::ErrorKind::PermissionDenied)) => {
                FileErrorCode::Denied
            }
            XferError::Path(_) => FileErrorCode::BadPath,
            XferError::Io(e) => match e.kind() {
                io::ErrorKind::NotFound => FileErrorCode::NotFound,
                io::ErrorKind::PermissionDenied => FileErrorCode::Denied,
                io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => FileErrorCode::NoSpace,
                _ => FileErrorCode::Io,
            },
            XferError::BadHeader | XferError::TooLong => FileErrorCode::Protocol,
            XferError::Incomplete { .. } => FileErrorCode::Cancelled,
            XferError::ResumeMismatch => FileErrorCode::ResumeMismatch,
            XferError::HashMismatch => FileErrorCode::HashMismatch,
            XferError::Changed => FileErrorCode::Changed,
        }
    }
}

impl fmt::Display for XferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            XferError::Path(e) => write!(f, "{e}"),
            XferError::Io(e) => write!(f, "erro de E/S: {e}"),
            XferError::BadHeader => write!(f, "cabeçalho do stream de dados inválido"),
            XferError::Incomplete { received } => {
                write!(f, "transferência interrompida em {received} bytes")
            }
            XferError::TooLong => write!(f, "o peer mandou mais bytes do que ofereceu"),
            XferError::ResumeMismatch => write!(f, "o arquivo parcial não bate com a origem"),
            XferError::HashMismatch => write!(f, "o BLAKE3 do arquivo recebido não confere"),
            XferError::Changed => write!(f, "o arquivo de origem mudou durante o envio"),
        }
    }
}

impl std::error::Error for XferError {}

impl From<io::Error> for XferError {
    fn from(e: io::Error) -> Self {
        XferError::Io(e)
    }
}

impl From<PathError> for XferError {
    fn from(e: PathError) -> Self {
        XferError::Path(e)
    }
}

/// Milissegundos desde 1970 de um `SystemTime` (0 se indisponível).
pub fn to_unix_ms(t: io::Result<SystemTime>) -> i64 {
    t.ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn from_unix_ms(ms: i64) -> Option<SystemTime> {
    let ms = u64::try_from(ms).ok().filter(|&ms| ms > 0)?;
    UNIX_EPOCH.checked_add(Duration::from_millis(ms))
}

/// Lê `len` bytes de `file` para dentro de `hasher`, reaproveitando `buf`.
async fn hash_prefix(
    file: &mut File,
    len: u64,
    hasher: &mut blake3::Hasher,
    buf: &mut [u8],
) -> io::Result<()> {
    let mut left = len;
    while left > 0 {
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let n = file.read(&mut buf[..want]).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        hasher.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(())
}

/// Arquivo a enviar.
#[derive(Debug)]
pub struct Source {
    file: File,
    size: u64,
    mtime_ms: i64,
}

impl Source {
    /// Abre `path` (já resolvido por quem chama: `resolve_in_root` para pedidos do peer).
    pub async fn open(path: &Path) -> Result<Source, XferError> {
        let file = File::open(path).await?;
        let meta = file.metadata().await?;
        if !meta.is_file() {
            return Err(XferError::Io(io::ErrorKind::InvalidInput.into()));
        }
        Ok(Source {
            file,
            size: meta.len(),
            mtime_ms: to_unix_ms(meta.modified()),
        })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn mtime_ms(&self) -> i64 {
        self.mtime_ms
    }

    /// Manda o arquivo a partir de `offset` por `out` e devolve o BLAKE3 do arquivo inteiro.
    ///
    /// `prefix` é o do `Accept`: se não bater com os primeiros `offset` bytes, nada é enviado e
    /// o erro é [`XferError::ResumeMismatch`]. `progress` recebe o total de bytes já no destino.
    /// Quem chama encerra o stream (`finish`) e manda o `Done`.
    pub async fn send<W: AsyncWrite + Unpin>(
        &mut self,
        xfer_id: u32,
        offset: u64,
        prefix: Option<Hash>,
        out: &mut W,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Hash, XferError> {
        if offset > self.size {
            return Err(XferError::ResumeMismatch);
        }
        let mut buf = vec![0u8; limits::CHUNK];
        let mut hasher = blake3::Hasher::new();
        self.file.rewind().await?;
        if offset > 0 {
            hash_prefix(&mut self.file, offset, &mut hasher, &mut buf)
                .await
                .map_err(|e| match e.kind() {
                    io::ErrorKind::UnexpectedEof => XferError::Changed,
                    _ => XferError::Io(e),
                })?;
            if prefix != Some(*hasher.finalize().as_bytes()) {
                return Err(XferError::ResumeMismatch);
            }
        }

        out.write_all(&DataHeader { xfer_id, offset }.encode())
            .await?;
        let mut sent = offset;
        progress(sent);
        while sent < self.size {
            let n = self.file.read(&mut buf).await?;
            if n == 0 {
                return Err(XferError::Changed);
            }
            if sent + n as u64 > self.size {
                return Err(XferError::Changed);
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).await?;
            sent += n as u64;
            progress(sent);
        }
        Ok(*hasher.finalize().as_bytes())
    }
}

/// Arquivo sendo recebido em `nome.part`.
#[derive(Debug)]
pub struct Incoming {
    target: PathBuf,
    part: PathBuf,
    file: File,
    size: u64,
    mtime_ms: i64,
    written: u64,
    hasher: blake3::Hasher,
    prefix: Option<Hash>,
}

impl Incoming {
    /// Prepara a recepção de `rel_path` (do peer) dentro de `root`.
    ///
    /// Cria as pastas que faltam e abre `rel_path.part`. Se ele já existe e não é maior que
    /// `size`, a transferência retoma do tamanho dele ([`Incoming::offset`]).
    pub async fn prepare(
        root: &Path,
        rel_path: &str,
        size: u64,
        mtime_ms: i64,
    ) -> Result<Incoming, XferError> {
        let target = resolve_in_root(root, rel_path)?;
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).await?;
        }
        // Resolve de novo: as pastas agora existem e não podem ter virado links para fora.
        let target = resolve_in_root(root, rel_path)?;
        let part = resolve_in_root(root, &format!("{rel_path}{}", crate::PART_SUFFIX))?;
        debug_assert_eq!(part, part_path(&target));

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part)
            .await?;
        let mut have = file.metadata().await?.len();
        if have > size {
            file.set_len(0).await?;
            have = 0;
        }

        let mut hasher = blake3::Hasher::new();
        let mut prefix = None;
        if have > 0 {
            let mut buf = vec![0u8; limits::CHUNK];
            hash_prefix(&mut file, have, &mut hasher, &mut buf).await?;
            prefix = Some(*hasher.finalize().as_bytes());
        }
        Ok(Incoming {
            target,
            part,
            file,
            size,
            mtime_ms,
            written: have,
            hasher,
            prefix,
        })
    }

    /// Onde a transferência retoma (vai no `Accept`).
    pub fn offset(&self) -> u64 {
        self.written
    }

    /// BLAKE3 dos bytes já gravados (vai no `Accept`; `None` se `offset` = 0).
    pub fn prefix(&self) -> Option<Hash> {
        self.prefix
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn part_path(&self) -> &Path {
        &self.part
    }

    /// Descarta o `.part` e recomeça do zero (depois de um `ResumeMismatch`).
    pub async fn restart(&mut self) -> Result<(), XferError> {
        self.file.set_len(0).await?;
        self.file.rewind().await?;
        self.written = 0;
        self.hasher = blake3::Hasher::new();
        self.prefix = None;
        Ok(())
    }

    /// Lê o stream de dados até o fim do arquivo, gravando no `.part`.
    ///
    /// Se o stream acabar antes, os bytes recebidos ficam gravados e o erro é
    /// [`XferError::Incomplete`]: basta uma nova oferta para retomar.
    pub async fn receive<R: tokio::io::AsyncRead + Unpin>(
        &mut self,
        xfer_id: u32,
        input: &mut R,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), XferError> {
        let mut head = [0u8; DataHeader::LEN];
        input
            .read_exact(&mut head)
            .await
            .map_err(|_| XferError::BadHeader)?;
        match DataHeader::decode(&head) {
            Some(h) if h.xfer_id == xfer_id && h.offset == self.written => {}
            _ => return Err(XferError::BadHeader),
        }
        self.file.seek(io::SeekFrom::Start(self.written)).await?;

        let mut buf = vec![0u8; limits::CHUNK];
        let result = loop {
            let left = self.size - self.written;
            if left == 0 {
                // O arquivo está completo: qualquer byte a mais é erro do peer.
                let extra = input.read(&mut buf[..1]).await;
                break match extra {
                    Ok(0) => Ok(()),
                    Ok(_) => Err(XferError::TooLong),
                    Err(e) => Err(XferError::Io(e)),
                };
            }
            let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
            let n = match input.read(&mut buf[..want]).await {
                Ok(0) | Err(_) => {
                    break Err(XferError::Incomplete {
                        received: self.written,
                    });
                }
                Ok(n) => n,
            };
            if let Err(e) = self.file.write_all(&buf[..n]).await {
                break Err(XferError::Io(e));
            }
            self.hasher.update(&buf[..n]);
            self.written += n as u64;
            progress(self.written);
        };
        // Garante no disco o que chegou, inclusive numa interrupção (é o que a retomada usa).
        self.file.flush().await?;
        result
    }

    /// Confere o BLAKE3 do `Done` e põe o arquivo no lugar sem sobrescrever nada:
    /// se o nome já existe, vira `nome (1).ext`. Devolve o caminho final.
    pub async fn finish(self, expected: Hash) -> Result<PathBuf, XferError> {
        if self.written != self.size {
            return Err(XferError::Incomplete {
                received: self.written,
            });
        }
        if *self.hasher.finalize().as_bytes() != expected {
            drop(self.file);
            let _ = fs::remove_file(&self.part).await;
            return Err(XferError::HashMismatch);
        }
        self.file.sync_all().await?;
        let file = self.file.into_std().await;
        let part = self.part;
        let target = self.target;
        let mtime = from_unix_ms(self.mtime_ms);
        tokio::task::spawn_blocking(move || {
            if let Some(t) = mtime {
                // Data de modificação é cortesia: um sistema de arquivos sem suporte não falha a cópia.
                let _ = file.set_modified(t);
            }
            drop(file);
            place_without_overwrite(&part, &target)
        })
        .await
        .map_err(|e| XferError::Io(io::Error::other(e)))?
    }
}

/// Move `part` para `target` ou para o primeiro `nome (n).ext` livre, sem nunca sobrescrever.
///
/// Usa link físico, que falha atomicamente se o destino existe; em sistemas de arquivos sem
/// links (FAT/exFAT) cai para `rename` depois de conferir que o destino está livre.
fn place_without_overwrite(part: &Path, target: &Path) -> Result<PathBuf, XferError> {
    let dir = target.parent().ok_or(XferError::Path(PathError::Empty))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(XferError::Path(PathError::Empty))?;
    loop {
        let candidate = unique_path(dir, name);
        match std::fs::hard_link(part, &candidate) {
            Ok(()) => {
                std::fs::remove_file(part)?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                if candidate.symlink_metadata().is_ok() {
                    continue;
                }
                std::fs::rename(part, &candidate)?;
                return Ok(candidate);
            }
        }
    }
}
