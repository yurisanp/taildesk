//! Instância única do app: abrir o TailDesk de novo (menu de aplicativos, atalho) só traz a
//! janela da instância que já está na bandeja, em vez de criar outra.
//!
//! - Linux: unix socket em `$XDG_RUNTIME_DIR/taildesk-<nome>.sock` (por usuário).
//! - Windows: evento nomeado `Local\TailDesk-<nome>` (por sessão de login).

use std::io;

/// Resultado de [`garantir`].
#[derive(Debug)]
pub enum Instancia {
    /// Esta é a primeira; mantenha o guarda vivo enquanto o app roda.
    Primeira(Guarda),
    /// Já havia uma aberta, que foi avisada para mostrar a janela. Esta deve sair.
    JaAberta,
}

/// Garante uma instância por usuário com o nome dado. `ao_abrir` roda (numa thread própria)
/// sempre que outra instância pedir para mostrar a janela.
pub fn garantir(nome: &str, ao_abrir: impl Fn() + Send + 'static) -> io::Result<Instancia> {
    imp::garantir(nome, Box::new(ao_abrir))
}

pub use imp::Guarda;

#[cfg(unix)]
mod imp {
    use std::io::{self, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::Duration;

    use super::Instancia;

    const PEDIDO: &[u8] = b"abrir\n";

    /// Remove o socket ao sair.
    #[derive(Debug)]
    pub struct Guarda(PathBuf);

    impl Drop for Guarda {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn caminho(nome: &str) -> PathBuf {
        match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(dir) if !dir.is_empty() => {
                PathBuf::from(dir).join(format!("taildesk-{nome}.sock"))
            }
            _ => {
                let usuario = std::env::var("USER").unwrap_or_default();
                std::env::temp_dir().join(format!("taildesk-{nome}-{usuario}.sock"))
            }
        }
    }

    pub(super) fn garantir(nome: &str, ao_abrir: Box<dyn Fn() + Send>) -> io::Result<Instancia> {
        let caminho = caminho(nome);
        if let Ok(mut s) = UnixStream::connect(&caminho) {
            s.set_write_timeout(Some(Duration::from_secs(2)))?;
            s.write_all(PEDIDO)?;
            return Ok(Instancia::JaAberta);
        }
        // Ninguém atendeu: socket velho de uma instância que caiu.
        let _ = std::fs::remove_file(&caminho);
        let ouvinte = UnixListener::bind(&caminho)?;
        std::thread::Builder::new()
            .name("taildesk-instancia".into())
            .spawn(move || {
                for conexao in ouvinte.incoming() {
                    let Ok(mut s) = conexao else { continue };
                    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = [0u8; 16];
                    let mut lido = 0;
                    while lido < PEDIDO.len() {
                        match s.read(&mut buf[lido..]) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => lido += n,
                        }
                    }
                    if &buf[..lido] == PEDIDO {
                        ao_abrir();
                    }
                }
            })?;
        Ok(Instancia::Primeira(Guarda(caminho)))
    }
}

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, INFINITE, SetEvent, WaitForSingleObject,
    };

    use super::Instancia;

    /// O evento fica aberto enquanto o app roda (a thread que espera nele também).
    #[derive(Debug)]
    pub struct Guarda(());

    /// HANDLE é um ponteiro; o evento pode ser usado de qualquer thread.
    struct Evento(HANDLE);
    // SAFETY: handles de evento do Win32 não têm afinidade de thread.
    unsafe impl Send for Evento {}

    pub(super) fn garantir(nome: &str, ao_abrir: Box<dyn Fn() + Send>) -> io::Result<Instancia> {
        let nome: Vec<u16> = format!("Local\\TailDesk-{nome}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: `nome` termina em NUL e vive até o fim da chamada; atributos nulos = padrão.
        let evento = unsafe { CreateEventW(std::ptr::null(), 0, 0, nome.as_ptr()) };
        if evento.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: lido logo após o CreateEventW, na mesma thread.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: `evento` é um handle válido que acabamos de abrir.
            unsafe {
                SetEvent(evento);
                CloseHandle(evento);
            }
            return Ok(Instancia::JaAberta);
        }
        let evento = Evento(evento);
        std::thread::Builder::new()
            .name("taildesk-instancia".into())
            .spawn(move || {
                let evento = evento;
                loop {
                    // SAFETY: o handle nunca é fechado enquanto o processo roda.
                    let r = unsafe { WaitForSingleObject(evento.0, INFINITE) };
                    if r != WAIT_OBJECT_0 {
                        break;
                    }
                    ao_abrir();
                }
            })?;
        Ok(Instancia::Primeira(Guarda(())))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn segunda_instancia_avisa_a_primeira() {
        let nome = format!("teste-{}", std::process::id());
        let (tx, rx) = mpsc::channel();
        let guarda = match garantir(&nome, move || tx.send(()).unwrap()).unwrap() {
            Instancia::Primeira(g) => g,
            Instancia::JaAberta => panic!("não devia haver outra"),
        };
        assert!(matches!(
            garantir(&nome, || {}).unwrap(),
            Instancia::JaAberta
        ));
        rx.recv_timeout(Duration::from_secs(5))
            .expect("primeira avisada");
        drop(guarda);
        // Depois que a primeira sai, a próxima vira a primeira.
        assert!(matches!(
            garantir(&nome, || {}).unwrap(),
            Instancia::Primeira(_)
        ));
    }
}
