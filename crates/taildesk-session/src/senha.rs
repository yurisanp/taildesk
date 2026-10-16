//! Acesso com senha (fase 4): quem sabe a senha deste computador entra sem o diálogo de
//! consentimento, para acesso sem ninguém na frente da tela.
//!
//! O host guarda só o hash argon2id (formato PHC, com sal próprio); a senha vem no `Hello`,
//! dentro do QUIC com o certificado do host conferido, e o controlador a guarda no cofre do
//! sistema. Senhas erradas demais de um mesmo dispositivo bloqueiam a conferência por um tempo.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

/// Menor senha aceita nas Configurações, em caracteres.
pub const MINIMO: usize = 8;
/// Erros seguidos de um dispositivo antes do bloqueio.
pub const ERROS_MAX: u32 = 5;
/// Quanto dura o bloqueio (contado do primeiro erro da série).
pub const BLOQUEIO: Duration = Duration::from_secs(5 * 60);

/// A senha não pôde virar hash (curta demais ou falha do gerador).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErroSenha {
    Curta,
    Hash,
}

impl std::fmt::Display for ErroSenha {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErroSenha::Curta => write!(f, "A senha precisa de pelo menos {MINIMO} caracteres"),
            ErroSenha::Hash => write!(f, "Não foi possível guardar a senha"),
        }
    }
}

impl std::error::Error for ErroSenha {}

/// O hash para gravar nas configurações do host.
pub fn gerar_hash(senha: &str) -> Result<String, ErroSenha> {
    if senha.chars().count() < MINIMO {
        return Err(ErroSenha::Curta);
    }
    let sal = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(senha.as_bytes(), &sal)
        .map(|h| h.to_string())
        .map_err(|_| ErroSenha::Hash)
}

/// A senha confere com o hash gravado (hash estragado = não confere).
pub fn confere(hash: &str, senha: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| {
        Argon2::default()
            .verify_password(senha.as_bytes(), &h)
            .is_ok()
    })
}

/// Erros de senha por dispositivo (`node_stable_id`).
#[derive(Debug, Default)]
pub struct Tentativas {
    erros: HashMap<String, (u32, Instant)>,
}

impl Tentativas {
    /// Errou demais e o bloqueio ainda vale.
    pub fn bloqueado(&mut self, node: &str, agora: Instant) -> bool {
        match self.erros.get(node) {
            Some(&(_, desde)) if agora.duration_since(desde) >= BLOQUEIO => {
                self.erros.remove(node);
                false
            }
            Some(&(n, _)) => n >= ERROS_MAX,
            None => false,
        }
    }

    pub fn errou(&mut self, node: &str, agora: Instant) {
        self.erros
            .entry(node.to_owned())
            .and_modify(|(n, _)| *n += 1)
            .or_insert((1, agora));
    }

    pub fn acertou(&mut self, node: &str) {
        self.erros.remove(node);
    }
}

/// As tentativas de todas as sessões deste processo.
pub(crate) fn tentativas<R>(f: impl FnOnce(&mut Tentativas) -> R) -> R {
    static T: Mutex<Option<Tentativas>> = Mutex::new(None);
    let mut g = T.lock().unwrap_or_else(PoisonError::into_inner);
    f(g.get_or_insert_with(Tentativas::default))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_confere_so_com_a_senha_certa() {
        let h = gerar_hash("cavalo correto").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(!h.contains("cavalo"));
        assert!(confere(&h, "cavalo correto"));
        assert!(!confere(&h, "cavalo errado"));
        assert!(!confere("lixo", "cavalo correto"));
        // Sal novo a cada vez.
        assert_ne!(h, gerar_hash("cavalo correto").unwrap());
        assert_eq!(gerar_hash("curta"), Err(ErroSenha::Curta));
    }

    #[test]
    fn bloqueia_depois_de_errar_demais() {
        let mut t = Tentativas::default();
        let t0 = Instant::now();
        for _ in 0..ERROS_MAX {
            assert!(!t.bloqueado("n1", t0));
            t.errou("n1", t0);
        }
        assert!(t.bloqueado("n1", t0 + Duration::from_secs(1)));
        assert!(!t.bloqueado("n2", t0));
        assert!(!t.bloqueado("n1", t0 + BLOQUEIO));
        t.errou("n2", t0);
        t.acertou("n2");
        assert!(t.erros.is_empty());
    }
}
