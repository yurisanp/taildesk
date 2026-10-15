//! Navegador remoto e envio de pastas: listar uma pasta dentro de uma raiz liberada e
//! transformar uma pasta local na lista de arquivos a oferecer.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use taildesk_proto::control;
use taildesk_proto::files::limits;

use crate::xfer::to_unix_ms;
use crate::{PathError, resolve_in_root};

/// Tipo de um item listado.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Dir,
    File,
    /// Dispositivo, socket etc.: aparece, mas não pode ser baixado.
    Other,
}

/// Um item da listagem de uma pasta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// Bytes; 0 para pastas.
    pub size: u64,
    /// Milissegundos desde 1970 (UTC); 0 se desconhecido.
    pub mtime_ms: i64,
}

/// Resolve `rel` dentro de `root`, aceitando `""` como a própria raiz.
pub fn resolve_dir(root: &Path, rel: &str) -> Result<PathBuf, PathError> {
    if rel.is_empty() {
        root.canonicalize().map_err(|e| PathError::Io(e.kind()))
    } else {
        resolve_in_root(root, rel)
    }
}

/// Lista a pasta `rel` de `root`: pastas primeiro, depois arquivos, por nome.
///
/// Links simbólicos aparecem pelo que apontam, mas só se o alvo ficar dentro da raiz; os que
/// saem dela e os nomes que não são UTF-8 válido ficam de fora.
pub fn list_dir(root: &Path, rel: &str) -> Result<Vec<Entry>, PathError> {
    let root = root.canonicalize().map_err(|e| PathError::Io(e.kind()))?;
    let dir = resolve_dir(&root, rel)?;
    let read = fs::read_dir(&dir).map_err(|e| PathError::Io(e.kind()))?;
    let mut out = Vec::new();
    for item in read {
        let Ok(item) = item else { continue };
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        if name.len() > limits::NAME_LEN {
            continue;
        }
        let path = item.path();
        let Ok(meta) = item.metadata() else { continue };
        let meta = if meta.file_type().is_symlink() {
            match path.canonicalize() {
                Ok(target) if target.starts_with(&root) => match fs::metadata(&target) {
                    Ok(m) => m,
                    Err(_) => continue,
                },
                _ => continue,
            }
        } else {
            meta
        };
        let kind = if meta.is_dir() {
            EntryKind::Dir
        } else if meta.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        out.push(Entry {
            name,
            kind,
            size: if kind == EntryKind::File {
                meta.len()
            } else {
                0
            },
            mtime_ms: to_unix_ms(meta.modified()),
        });
    }
    out.sort_by(|a, b| {
        (a.kind != EntryKind::Dir)
            .cmp(&(b.kind != EntryKind::Dir))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

/// Uma página da listagem a partir de `start`, com `more` = há itens depois dela.
///
/// Para em [`limits::LISTING_ENTRIES`] itens ou [`limits::LISTING_BYTES`] de nomes, o que vier
/// primeiro, para o `FsListing` caber num frame de controle. Sempre leva ao menos um item.
pub fn page(entries: &[Entry], start: u32) -> (&[Entry], bool) {
    let start = (start as usize).min(entries.len());
    let mut end = start;
    let mut bytes = 0;
    while end < entries.len() && end - start < limits::LISTING_ENTRIES {
        bytes += entries[end].name.len();
        if bytes > limits::LISTING_BYTES && end > start {
            break;
        }
        end += 1;
    }
    (&entries[start..end], end < entries.len())
}

impl From<&Entry> for control::FsEntry {
    fn from(e: &Entry) -> control::FsEntry {
        let kind = match e.kind {
            EntryKind::Dir => control::FsEntryKind::Dir,
            EntryKind::File => control::FsEntryKind::File,
            EntryKind::Other => control::FsEntryKind::Other,
        };
        control::FsEntry {
            name: e.name.clone(),
            kind: kind.into(),
            size: e.size,
            mtime_ms: e.mtime_ms,
        }
    }
}

/// Um arquivo de uma pasta a enviar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkFile {
    /// Caminho para o `Offer`: começa pelo nome da pasta e usa `/`.
    pub rel_path: String,
    pub path: PathBuf,
    pub size: u64,
}

/// Resultado de [`walk`].
#[derive(Debug, Default)]
pub struct Walk {
    pub files: Vec<WalkFile>,
    /// Itens que não podem ir (links simbólicos, nomes não UTF-8 ou longos demais).
    pub skipped: Vec<PathBuf>,
}

/// Lista recursivamente os arquivos de `dir` para enviar a pasta inteira. Não segue links.
pub fn walk(dir: &Path) -> io::Result<Walk> {
    let base = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?
        .to_owned();
    let mut out = Walk::default();
    let mut stack = vec![(dir.to_path_buf(), base)];
    while let Some((path, rel)) = stack.pop() {
        let mut items: Vec<_> = fs::read_dir(&path)?.filter_map(Result::ok).collect();
        items.sort_by_key(|i| i.file_name());
        for item in items {
            let child = item.path();
            let Ok(name) = item.file_name().into_string() else {
                out.skipped.push(child);
                continue;
            };
            let child_rel = format!("{rel}/{name}");
            let Ok(kind) = item.file_type() else {
                out.skipped.push(child);
                continue;
            };
            if kind.is_symlink() || child_rel.len() > limits::PATH_LEN {
                out.skipped.push(child);
            } else if kind.is_dir() {
                stack.push((child, child_rel));
            } else if kind.is_file() {
                let size = item.metadata()?.len();
                out.files.push(WalkFile {
                    rel_path: child_rel,
                    path: child,
                    size,
                });
            } else {
                out.skipped.push(child);
            }
        }
    }
    out.files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}
