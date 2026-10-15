//! Troca de arquivos (`docs/context/10-plano-windows-linux.md` §3).
//!
//! A raiz do crate concentra a parte de segurança: todo caminho que chega do peer é **relativo a
//! uma raiz permitida**, usa `/` como separador e é validado aqui antes de tocar o disco.
//!
//! - [`xfer`]: envio e recepção dos bytes (stream de dados, BLAKE3, `.part`, retomada);
//! - [`listing`]: listagem para o navegador remoto e envio de pastas;
//! - [`service`]: sessão de arquivos (mensagens 80–89, streams, vagas, permissões);
//! - [`audit`]: registro local de cada transferência;
//! - [`panel`]: estado da faixa de transferências e do navegador do host, para a interface.
//!
//! As mensagens de controle estão em `taildesk_proto::files`.

use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

pub mod audit;
pub mod listing;
pub mod panel;
pub mod service;
pub mod xfer;

pub use xfer::{Incoming, Source, XferError};

/// Sufixo dos arquivos em transferência; o rename para o nome final é atômico no fim.
pub const PART_SUFFIX: &str = ".part";

/// Motivo de um caminho recebido do peer ser recusado.
#[derive(Debug, PartialEq, Eq)]
pub enum PathError {
    Empty,
    Absolute,
    /// Componente `.`, `..` ou vazio (`a//b`).
    BadComponent(String),
    /// Nome reservado do Windows (`CON`, `NUL`, `COM1`...).
    Reserved(String),
    /// Caractere de controle ou proibido no Windows (`<>:"\|?*`).
    BadChar(String),
    /// Termina em ponto ou espaço (o Windows descarta em silêncio).
    TrailingDotOrSpace(String),
    /// O caminho resolvido sai da raiz (link simbólico).
    EscapesRoot,
    Io(io::ErrorKind),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::Empty => write!(f, "caminho vazio"),
            PathError::Absolute => write!(f, "caminho absoluto não é permitido"),
            PathError::BadComponent(c) => write!(f, "componente inválido: {c:?}"),
            PathError::Reserved(c) => write!(f, "nome reservado: {c:?}"),
            PathError::BadChar(c) => write!(f, "caractere proibido em {c:?}"),
            PathError::TrailingDotOrSpace(c) => write!(f, "nome termina em ponto ou espaço: {c:?}"),
            PathError::EscapesRoot => write!(f, "caminho sai da pasta permitida"),
            PathError::Io(k) => write!(f, "erro de E/S: {k}"),
        }
    }
}

impl std::error::Error for PathError {}

const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];

fn is_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return true;
    }
    // COM0-9 e LPT0-9, incluindo os dígitos sobrescritos que o Windows também reserva.
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: Vec<char> = chars.collect();
    (prefix == "COM" || prefix == "LPT")
        && rest.len() == 1
        && (rest[0].is_ascii_digit() || matches!(rest[0], '¹' | '²' | '³'))
}

/// Valida um caminho relativo recebido do peer (separador `/`) e o converte para o SO local.
///
/// As regras são as do Windows mesmo no Linux, para que um arquivo aceito num lado sempre
/// possa ser gravado no outro.
pub fn sanitize_relative(rel: &str) -> Result<PathBuf, PathError> {
    if rel.is_empty() {
        return Err(PathError::Empty);
    }
    if rel.starts_with('/') || rel.starts_with('\\') || rel.as_bytes().get(1) == Some(&b':') {
        return Err(PathError::Absolute);
    }
    let mut out = PathBuf::new();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(PathError::BadComponent(part.to_owned()));
        }
        if part
            .chars()
            .any(|c| c.is_control() || "<>:\"\\|?*".contains(c))
        {
            return Err(PathError::BadChar(part.to_owned()));
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err(PathError::TrailingDotOrSpace(part.to_owned()));
        }
        if is_reserved(part) {
            return Err(PathError::Reserved(part.to_owned()));
        }
        out.push(part);
    }
    Ok(out)
}

/// Resolve `rel` dentro de `root`, recusando caminhos que escapem da raiz por links simbólicos.
///
/// `root` precisa existir. Partes de `rel` que ainda não existem (pastas a criar, o arquivo
/// novo) são permitidas; a parte que já existe é canonicalizada e precisa ficar dentro da raiz.
pub fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, PathError> {
    let rel = sanitize_relative(rel)?;
    let root = root.canonicalize().map_err(|e| PathError::Io(e.kind()))?;
    let target = root.join(&rel);

    let mut existing = target.as_path();
    let mut missing = Vec::new();
    loop {
        match existing.symlink_metadata() {
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or(PathError::EscapesRoot)?;
                missing.push(name.to_owned());
                existing = existing.parent().ok_or(PathError::EscapesRoot)?;
            }
            Err(e) => return Err(PathError::Io(e.kind())),
        }
    }
    let resolved = existing
        .canonicalize()
        .map_err(|e| PathError::Io(e.kind()))?;
    if !resolved.starts_with(&root) {
        return Err(PathError::EscapesRoot);
    }
    let mut out = resolved;
    for name in missing.into_iter().rev() {
        out.push(name);
    }
    debug_assert!(out.components().all(|c| !matches!(c, Component::ParentDir)));
    Ok(out)
}

/// Primeiro nome livre em `dir` no estilo `arquivo (1).txt`, para nunca sobrescrever.
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !exists(&candidate) {
        return candidate;
    }
    let (stem, ext) = match file_name.rfind('.') {
        Some(i) if i > 0 => (&file_name[..i], &file_name[i..]),
        _ => (file_name, ""),
    };
    (1u32..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !exists(p))
        .expect("espaço de nomes esgotado")
}

fn exists(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

/// Caminho temporário usado durante a transferência (`nome.ext.part`).
pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(PART_SUFFIX);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("taildesk-files-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn accepts_normal_paths() {
        assert_eq!(
            sanitize_relative("a/b/c.txt").unwrap(),
            Path::new("a").join("b").join("c.txt")
        );
        assert!(sanitize_relative("Relatório de vendas.xlsx").is_ok());
        assert!(sanitize_relative(".bashrc").is_ok());
        assert!(sanitize_relative("console.log").is_ok());
    }

    #[test]
    fn rejects_traversal_and_absolute() {
        assert_eq!(sanitize_relative(""), Err(PathError::Empty));
        assert_eq!(sanitize_relative("/etc/passwd"), Err(PathError::Absolute));
        assert_eq!(
            sanitize_relative("\\\\server\\share"),
            Err(PathError::Absolute)
        );
        assert_eq!(sanitize_relative("C:/Windows"), Err(PathError::Absolute));
        assert!(matches!(
            sanitize_relative("a/../../b"),
            Err(PathError::BadComponent(_))
        ));
        assert!(matches!(
            sanitize_relative("a//b"),
            Err(PathError::BadComponent(_))
        ));
        assert!(matches!(
            sanitize_relative("./a"),
            Err(PathError::BadComponent(_))
        ));
        assert!(matches!(
            sanitize_relative("a\\..\\b"),
            Err(PathError::BadChar(_))
        ));
    }

    #[test]
    fn rejects_windows_hostile_names() {
        for name in ["CON", "nul.txt", "com1", "LPT9.log", "COM¹", "aux .txt"] {
            assert!(
                matches!(sanitize_relative(name), Err(PathError::Reserved(_))),
                "{name}"
            );
        }
        assert!(matches!(
            sanitize_relative("ab:c"),
            Err(PathError::BadChar(_))
        ));
        assert!(matches!(
            sanitize_relative("x\u{0}y"),
            Err(PathError::BadChar(_))
        ));
        assert!(matches!(
            sanitize_relative("nome."),
            Err(PathError::TrailingDotOrSpace(_))
        ));
        assert!(matches!(
            sanitize_relative("nome "),
            Err(PathError::TrailingDotOrSpace(_))
        ));
    }

    #[test]
    fn resolves_inside_root_including_missing_parts() {
        let tmp = TempDir::new("resolve");
        fs::create_dir(tmp.0.join("existe")).unwrap();
        let p = resolve_in_root(&tmp.0, "existe/nova/arquivo.bin").unwrap();
        assert!(p.starts_with(tmp.0.canonicalize().unwrap()));
        assert!(p.ends_with(Path::new("existe").join("nova").join("arquivo.bin")));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let tmp = TempDir::new("symlink");
        let root = tmp.0.join("root");
        let outside = tmp.0.join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert_eq!(
            resolve_in_root(&root, "link/x.txt"),
            Err(PathError::EscapesRoot)
        );
    }

    #[test]
    fn unique_path_never_overwrites() {
        let tmp = TempDir::new("unique");
        assert_eq!(unique_path(&tmp.0, "a.txt"), tmp.0.join("a.txt"));
        fs::write(tmp.0.join("a.txt"), b"").unwrap();
        fs::write(tmp.0.join("a (1).txt"), b"").unwrap();
        assert_eq!(unique_path(&tmp.0, "a.txt"), tmp.0.join("a (2).txt"));
        fs::write(tmp.0.join("LEIAME"), b"").unwrap();
        assert_eq!(unique_path(&tmp.0, "LEIAME"), tmp.0.join("LEIAME (1)"));
        fs::write(tmp.0.join(".env"), b"").unwrap();
        assert_eq!(unique_path(&tmp.0, ".env"), tmp.0.join(".env (1)"));
    }

    #[test]
    fn part_suffix() {
        assert_eq!(
            part_path(Path::new("x/a.tar.gz")),
            Path::new("x/a.tar.gz.part")
        );
    }
}
