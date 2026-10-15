//! Segredos no cofre do sistema (`keyring`): Secret Service no Linux (KWallet, GNOME Keyring),
//! Credential Manager no Windows. Nada disso vai para arquivo nem para log.
//!
//! Bloqueia (D-Bus ou Win32): chame fora da thread da interface.

use std::fmt;
use std::sync::Once;

use keyring_core::{Entry, Error};

const SERVICO: &str = "TailDesk";

/// O cofre não respondeu ou recusou.
#[derive(Debug)]
pub struct ErroCofre(String);

impl fmt::Display for ErroCofre {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cofre de senhas do sistema: {}", self.0)
    }
}

impl std::error::Error for ErroCofre {}

fn erro(e: Error) -> ErroCofre {
    ErroCofre(e.to_string())
}

fn entrada(nome: &str) -> Result<Entry, ErroCofre> {
    static INICIO: Once = Once::new();
    INICIO.call_once(|| {
        #[cfg(target_os = "linux")]
        let store = zbus_secret_service_keyring_store::Store::new();
        #[cfg(windows)]
        let store = windows_native_keyring_store::Store::new();
        match store {
            Ok(s) => keyring_core::set_default_store(s),
            Err(e) => eprintln!("taildesk: cofre de senhas indisponível ({e})"),
        }
    });
    Entry::new(SERVICO, nome).map_err(erro)
}

/// O segredo `nome`, ou `None` se ainda não existe.
pub fn ler(nome: &str) -> Result<Option<Vec<u8>>, ErroCofre> {
    match entrada(nome)?.get_secret() {
        Ok(s) => Ok(Some(s)),
        Err(Error::NoEntry) => Ok(None),
        Err(e) => Err(erro(e)),
    }
}

/// Grava (ou troca) o segredo `nome`.
pub fn gravar(nome: &str, segredo: &[u8]) -> Result<(), ErroCofre> {
    entrada(nome)?.set_secret(segredo).map_err(erro)
}
