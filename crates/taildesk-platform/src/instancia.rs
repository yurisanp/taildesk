//! Instância única do app: abrir o TailDesk de novo (menu de aplicativos, atalho) só traz a
//! janela da instância que já está na bandeja, em vez de criar outra.
//!
//! - Linux: unix socket em `$XDG_RUNTIME_DIR/taildesk-<nome>.sock` (por usuário). A segunda
//!   instância repassa o token de ativação que recebeu do lançador (xdg-activation no Wayland,
//!   startup-notification no X11), para a janela da primeira ganhar o foco do jeito nativo.
//! - Windows: evento nomeado `Local\TailDesk-<nome>` (por sessão de login). A segunda instância
//!   libera o primeiro plano (`AllowSetForegroundWindow`) antes de avisar.

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
/// sempre que outra instância pedir para mostrar a janela, com o token de ativação que ela
/// recebeu do sistema (ver [`crate::tomar_token_de_ativacao`]). `token` é o desta instância,
/// repassado à primeira quando esta não é a primeira.
pub fn garantir(
    nome: &str,
    token: Option<&str>,
    ao_abrir: impl Fn(Option<String>) + Send + 'static,
) -> io::Result<Instancia> {
    imp::garantir(nome, token, Box::new(ao_abrir))
}

type AoAbrir = Box<dyn Fn(Option<String>) + Send>;

pub use imp::Guarda;

#[cfg(unix)]
mod imp {
    use std::io::{self, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{AoAbrir, Instancia};

    const PEDIDO: &str = "abrir";
    /// Maior pedido aceito ("abrir <token>\n"); tokens de ativação têm algumas dezenas de bytes.
    const MAX_PEDIDO: usize = 1024;

    /// "abrir\n" ou "abrir <token>\n" → `Some(token)`; outra coisa → `None`.
    pub(super) fn ler_pedido(linha: &[u8]) -> Option<Option<String>> {
        let linha = std::str::from_utf8(linha).ok()?.strip_suffix('\n')?;
        let resto = linha.strip_prefix(PEDIDO)?;
        if resto.is_empty() {
            return Some(None);
        }
        let token = resto.strip_prefix(' ')?;
        let valido = !token.is_empty() && token.bytes().all(|b| b.is_ascii_graphic());
        valido.then(|| Some(token.to_owned()))
    }

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

    pub(super) fn garantir(
        nome: &str,
        token: Option<&str>,
        ao_abrir: AoAbrir,
    ) -> io::Result<Instancia> {
        let caminho = caminho(nome);
        if let Ok(mut s) = UnixStream::connect(&caminho) {
            s.set_write_timeout(Some(Duration::from_secs(2)))?;
            let pedido = match token {
                Some(t) => format!("{PEDIDO} {t}\n"),
                None => format!("{PEDIDO}\n"),
            };
            s.write_all(pedido.as_bytes())?;
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
                    let mut buf = [0u8; MAX_PEDIDO];
                    let mut lido = 0;
                    while lido < MAX_PEDIDO && !buf[..lido].contains(&b'\n') {
                        match s.read(&mut buf[lido..]) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => lido += n,
                        }
                    }
                    if let Some(token) = ler_pedido(&buf[..lido]) {
                        ao_abrir(token);
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
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

    use super::{AoAbrir, Instancia};

    /// O evento fica aberto enquanto o app roda (a thread que espera nele também).
    #[derive(Debug)]
    pub struct Guarda(());

    /// HANDLE é um ponteiro; o evento pode ser usado de qualquer thread.
    struct Evento(HANDLE);
    // SAFETY: handles de evento do Win32 não têm afinidade de thread.
    unsafe impl Send for Evento {}

    pub(super) fn garantir(
        nome: &str,
        _token: Option<&str>,
        ao_abrir: AoAbrir,
    ) -> io::Result<Instancia> {
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
            // SAFETY: sem ponteiros; deixa a primeira instância trazer a janela para a frente,
            // já que quem acabou de ser aberto pela pessoa é esta. `evento` é um handle válido
            // que acabamos de abrir.
            unsafe {
                AllowSetForegroundWindow(ASFW_ANY);
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
                    ao_abrir(None);
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
        let guarda = match garantir(&nome, None, move |t| tx.send(t).unwrap()).unwrap() {
            Instancia::Primeira(g) => g,
            Instancia::JaAberta => panic!("não devia haver outra"),
        };
        let ja = garantir(&nome, Some("kwin-123_abc"), |_| {}).unwrap();
        assert!(matches!(ja, Instancia::JaAberta));
        let token = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("primeira avisada");
        assert_eq!(token.as_deref(), Some("kwin-123_abc"));
        let ja = garantir(&nome, None, |_| {}).unwrap();
        assert!(matches!(ja, Instancia::JaAberta));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        drop(guarda);
        // Depois que a primeira sai, a próxima vira a primeira.
        let nova = garantir(&nome, None, |_| {}).unwrap();
        assert!(matches!(nova, Instancia::Primeira(_)));
    }

    #[test]
    fn pedido_com_e_sem_token() {
        use super::imp::ler_pedido;
        assert_eq!(ler_pedido(b"abrir\n"), Some(None));
        assert_eq!(ler_pedido(b"abrir t0k3n\n"), Some(Some("t0k3n".into())));
        assert_eq!(ler_pedido(b"abrir"), None);
        assert_eq!(ler_pedido(b"abrir \n"), None);
        assert_eq!(ler_pedido(b"abrir a b\n"), None);
        assert_eq!(ler_pedido(b"fechar\n"), None);
    }
}
