//! "Compartilhar com TailDesk" (MF-63): arquivos que chegam pelo menu de compartilhar do
//! Android vão para a pasta de entrada de um computador sem abrir a tela da sessão.
//!
//! A conexão é uma sessão comum (o host aplica as mesmas regras e pode pedir aceite), mas o
//! celular não pede vídeo: quando ela fica ao vivo, oferece os arquivos, conta o que terminou e
//! encerra sozinho. Aqui fica a contagem; a ligação com as telas está em `app::folha`.

use crate::arquivos::{Escolhido, LIMITE_TARIFADA};
use crate::textos::Idioma;

/// Um envio sem sessão em andamento.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Envio {
    /// Ainda não oferecidos (a sessão não ficou ao vivo).
    pub arquivos: Vec<Escolhido>,
    total: usize,
    ok: usize,
    falhas: usize,
}

/// Como acabou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resumo {
    pub total: usize,
    pub ok: usize,
}

impl Envio {
    pub fn novo(arquivos: Vec<Escolhido>) -> Envio {
        Envio {
            total: arquivos.len(),
            arquivos,
            ok: 0,
            falhas: 0,
        }
    }

    /// Já ofereceu os arquivos ao host.
    pub fn comecou(&self) -> bool {
        self.arquivos.is_empty()
    }

    /// Os arquivos para oferecer, uma vez só.
    pub fn comecar(&mut self) -> Vec<Escolhido> {
        std::mem::take(&mut self.arquivos)
    }

    /// Um arquivo terminou (`ok`) ou falhou. `Some` quando foi o último.
    pub fn terminou(&mut self, ok: bool) -> Option<Resumo> {
        if ok {
            self.ok += 1;
        } else {
            self.falhas += 1;
        }
        (self.ok + self.falhas >= self.total).then_some(Resumo {
            total: self.total,
            ok: self.ok,
        })
    }
}

impl Resumo {
    pub fn texto(&self, idioma: Idioma, nome: &str) -> String {
        match (self.ok, self.total) {
            (ok, total) if ok == total && total == 1 => {
                format!(
                    "{} {nome}.",
                    idioma.t("Arquivo enviado para", "File sent to")
                )
            }
            (ok, total) if ok == total => {
                format!(
                    "{total} {} {nome}.",
                    idioma.t("arquivos enviados para", "files sent to")
                )
            }
            (0, _) => format!(
                "{} {nome}.",
                idioma.t("Nada foi enviado para", "Nothing was sent to")
            ),
            (ok, total) => format!(
                "{ok} {} {total} {} {nome}.",
                idioma.t("de", "of"),
                idioma.t("arquivos enviados para", "files sent to")
            ),
        }
    }
}

/// Faixa da lista de dispositivos enquanto a pessoa escolhe o computador.
pub fn faixa(arquivos: &[Escolhido], tarifada: bool, idioma: Idioma) -> String {
    let tamanho: u64 = arquivos.iter().map(|a| a.tamanho).sum();
    let l = match idioma {
        Idioma::Pt => taildesk_files::panel::fmt::Locale::PtBr,
        Idioma::En => taildesk_files::panel::fmt::Locale::En,
    };
    let quantos = if arquivos.len() == 1 {
        arquivos[0].nome.clone()
    } else {
        format!("{} {}", arquivos.len(), idioma.t("arquivos", "files"))
    };
    let mut t = format!(
        "{} {quantos} ({}).",
        idioma.t(
            "Escolha o computador para receber",
            "Choose the computer to receive"
        ),
        taildesk_files::panel::fmt::size(tamanho, l)
    );
    if tarifada && tamanho > LIMITE_TARIFADA {
        t.push(' ');
        t.push_str(idioma.t(
            "Você está numa rede tarifada.",
            "You are on a metered network.",
        ));
    }
    t
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn arq(nome: &str, tamanho: u64) -> Escolhido {
        Escolhido {
            caminho: PathBuf::from("/cache").join(nome),
            nome: nome.into(),
            tamanho,
        }
    }

    #[test]
    fn conta_ate_o_ultimo() {
        let mut e = Envio::novo(vec![arq("a", 1), arq("b", 2), arq("c", 3)]);
        assert!(!e.comecou());
        assert_eq!(e.comecar().len(), 3);
        assert!(e.comecou());
        assert!(e.comecar().is_empty());
        assert_eq!(e.terminou(true), None);
        assert_eq!(e.terminou(false), None);
        let r = e.terminou(true).unwrap();
        assert_eq!(r, Resumo { total: 3, ok: 2 });
        assert_eq!(
            r.texto(Idioma::Pt, "pc"),
            "2 de 3 arquivos enviados para pc."
        );
        assert_eq!(
            Resumo { total: 1, ok: 1 }.texto(Idioma::En, "pc"),
            "File sent to pc."
        );
        assert_eq!(
            Resumo { total: 2, ok: 0 }.texto(Idioma::Pt, "pc"),
            "Nada foi enviado para pc."
        );
    }

    #[test]
    fn faixa_avisa_da_rede_tarifada() {
        let um = [arq("foto.jpg", 2048)];
        assert_eq!(
            faixa(&um, true, Idioma::Pt),
            "Escolha o computador para receber foto.jpg (2,0 KB)."
        );
        let grande = [arq("a.iso", LIMITE_TARIFADA), arq("b", 1)];
        assert!(faixa(&grande, true, Idioma::Pt).contains("2 arquivos"));
        assert!(faixa(&grande, true, Idioma::Pt).ends_with("rede tarifada."));
        assert!(!faixa(&grande, false, Idioma::En).contains("metered"));
    }
}
