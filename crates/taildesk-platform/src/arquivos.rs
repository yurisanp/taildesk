//! Diálogos de arquivo do sistema para o painel de arquivos da sessão:
//! - Linux: portal FileChooser e OpenURI (`xdg-desktop-portal`), nativos no Wayland e no X11
//!   e dentro de Flatpak;
//! - Windows: o diálogo comum do Explorer (`IFileOpenDialog`, pelo `rfd`) e o Explorer para
//!   abrir a pasta.

use std::io;
use std::path::{Path, PathBuf};

/// Pede ao usuário arquivos (ou, com `pastas`, pastas) para enviar. Vazio = cancelou.
pub async fn escolher(titulo: &str, pastas: bool) -> io::Result<Vec<PathBuf>> {
    #[cfg(target_os = "linux")]
    {
        linux::escolher(titulo, pastas).await
    }
    #[cfg(windows)]
    {
        let d = rfd::AsyncFileDialog::new().set_title(titulo);
        let escolhidos = if pastas {
            d.pick_folders().await
        } else {
            d.pick_files().await
        };
        Ok(escolhidos
            .unwrap_or_default()
            .iter()
            .map(|h| h.path().to_path_buf())
            .collect())
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (titulo, pastas);
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// Abre a pasta no gerenciador de arquivos.
pub async fn abrir_pasta(dir: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::abrir_pasta(dir).await
    }
    #[cfg(windows)]
    {
        use std::process::{Command, Stdio};
        let mut filho = Command::new("explorer.exe")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // O Explorer devolve 1 mesmo quando abre; só recolhe o processo.
        std::thread::spawn(move || filho.wait());
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = dir;
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// `file:///home/y/a%20b.txt` → `/home/y/a b.txt`. Outros esquemas (e `file://host/`) dão `None`.
pub fn caminho_de_uri(uri: &str) -> Option<PathBuf> {
    let resto = uri.strip_prefix("file://")?;
    if !resto.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(resto.len());
    let mut it = resto.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex = [it.next()?, it.next()?];
            let s = std::str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(s, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io;
    use std::path::{Path, PathBuf};

    use ashpd::desktop::ResponseError;
    use ashpd::desktop::file_chooser::SelectedFiles;
    use ashpd::desktop::open_uri::OpenDirectoryRequest;

    fn erro(e: ashpd::Error) -> io::Error {
        io::Error::other(e)
    }

    pub(super) async fn escolher(titulo: &str, pastas: bool) -> io::Result<Vec<PathBuf>> {
        let pedido = SelectedFiles::open_file()
            .title(titulo)
            .modal(true)
            .multiple(true)
            .directory(pastas)
            .send()
            .await
            .map_err(erro)?;
        match pedido.response() {
            Ok(r) => Ok(r
                .uris()
                .iter()
                .filter_map(|u| super::caminho_de_uri(u.as_str()))
                .collect()),
            Err(ashpd::Error::Response(ResponseError::Cancelled)) => Ok(Vec::new()),
            Err(e) => Err(erro(e)),
        }
    }

    pub(super) async fn abrir_pasta(dir: &Path) -> io::Result<()> {
        let pasta = std::fs::File::open(dir)?;
        OpenDirectoryRequest::default()
            .send(&pasta)
            .await
            .map_err(erro)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::caminho_de_uri;
    use std::path::PathBuf;

    #[test]
    fn uri_de_arquivo() {
        assert_eq!(
            caminho_de_uri("file:///home/y/Relat%C3%B3rio%20final.pdf"),
            Some(PathBuf::from("/home/y/Relatório final.pdf"))
        );
        assert_eq!(caminho_de_uri("file:///a/b"), Some(PathBuf::from("/a/b")));
        assert_eq!(caminho_de_uri("https://x/y"), None);
        assert_eq!(caminho_de_uri("file://host/a"), None);
        assert_eq!(caminho_de_uri("file:///a%2"), None);
        assert_eq!(caminho_de_uri("file:///a%zz"), None);
    }
}
