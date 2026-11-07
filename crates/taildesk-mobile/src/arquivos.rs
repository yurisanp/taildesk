//! Folha de arquivos da sessão no celular (MF-60..62, MF-64), sem depender da UI.
//!
//! Usa o mesmo estado do painel do desktop (`taildesk_files::panel`: [`Browser`] para navegar
//! nas pastas que o host libera e [`Transfers`] para a faixa de transferências) e acrescenta o que
//! é do celular:
//! - a lista única da folha: raízes do host ou o conteúdo da pasta aberta;
//! - a confirmação de arquivos grandes em rede tarifada (MF-64);
//! - as cópias temporárias do que veio do seletor do sistema (SAF), apagadas ao fim do envio;
//! - o destino do envio: a pasta aberta, se o host deixa escrever nela, ou a pasta de entrada.
//!
//! As regras de arquivo (`.part` + rename, BLAKE3, retomada, nunca sobrescrever,
//! `resolve_in_root`) ficam no serviço de arquivos (`taildesk_files::service`), igual ao desktop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use taildesk_files::audit::Direction;
use taildesk_files::panel::{Browser, ListRequest, State, Transfers};
use taildesk_files::service::Event;
use taildesk_proto::control::FsEntryKind;
use taildesk_proto::files::INBOX_ROOT;

/// Acima disto, em rede tarifada, o celular pergunta antes de baixar ou enviar (MF-64).
pub const LIMITE_TARIFADA: u64 = 50 * 1024 * 1024;

/// Uma linha da lista da folha.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub nome: String,
    pub pasta: bool,
    /// 0 para pastas e raízes.
    pub tamanho: u64,
    pub mtime_ms: i64,
    /// Raiz sem leitura: aparece, mas não abre.
    pub permitido: bool,
}

/// Um arquivo que veio do seletor do sistema, já copiado para uma pasta do app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Escolhido {
    pub caminho: PathBuf,
    pub nome: String,
    pub tamanho: u64,
}

/// O que espera o "sim" da pessoa (ou já foi liberado).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pedido {
    Baixar {
        root: u32,
        caminho: String,
        nome: String,
        tamanho: u64,
    },
    /// Para `root`, em `base` (`""` = a raiz).
    Enviar {
        root: u32,
        base: String,
        arquivos: Vec<Escolhido>,
    },
}

impl Pedido {
    pub fn tamanho(&self) -> u64 {
        match self {
            Pedido::Baixar { tamanho, .. } => *tamanho,
            Pedido::Enviar { arquivos, .. } => arquivos.iter().map(|a| a.tamanho).sum(),
        }
    }

    /// Nome para a pergunta ("relatorio.pdf", "3 arquivos").
    pub fn nome(&self, idioma: crate::textos::Idioma) -> String {
        match self {
            Pedido::Baixar { nome, .. } => nome.clone(),
            Pedido::Enviar { arquivos, .. } if arquivos.len() == 1 => arquivos[0].nome.clone(),
            Pedido::Enviar { arquivos, .. } => {
                format!("{} {}", arquivos.len(), idioma.t("arquivos", "files"))
            }
        }
    }

    /// Cópias temporárias, para apagar se a pessoa desistir.
    pub fn temporarios(&self) -> Vec<PathBuf> {
        match self {
            Pedido::Baixar { .. } => Vec::new(),
            Pedido::Enviar { arquivos, .. } => arquivos.iter().map(|a| a.caminho.clone()).collect(),
        }
    }
}

/// O que o app faz depois de um toque ou de um evento.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Acao {
    Nada,
    /// Pedir uma página ao host (`Files::list`).
    Listar(ListRequest),
    /// Fazer agora (`Files::download` ou `Files::send_file`).
    Fazer(Pedido),
    /// Mostrar a pergunta de [`Folha::pergunta`].
    Perguntar,
}

/// O que um evento do serviço pede ao app, além de redesenhar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Efeito {
    pub redesenhar: bool,
    /// Próxima página da listagem.
    pub listar: Option<ListRequest>,
    /// Download terminado: mandar para a pasta pública (`Download/TailDesk`).
    pub publicar: Option<(PathBuf, String)>,
    /// Cópia temporária de um envio que terminou (bem ou mal).
    pub apagar: Option<PathBuf>,
}

/// Estado da folha de uma sessão.
#[derive(Debug, Default)]
pub struct Folha {
    pub navegador: Browser,
    pub transferencias: Transfers,
    pergunta: Option<Pedido>,
    temporarios: HashMap<u32, PathBuf>,
    /// Cópias temporárias de envios desistidos, esperando o app apagar ([`Folha::lixo`]).
    lixo: Vec<PathBuf>,
}

impl Folha {
    pub fn new() -> Self {
        Self::default()
    }

    /// Raízes (nenhuma aberta) ou o conteúdo da pasta aberta (pastas primeiro, na ordem do
    /// host). Dispositivos e sockets não aparecem.
    pub fn itens(&self) -> Vec<Item> {
        if self.navegador.root().is_none() {
            return self
                .navegador
                .roots()
                .iter()
                .map(|r| Item {
                    nome: r.name.clone(),
                    pasta: true,
                    tamanho: 0,
                    mtime_ms: 0,
                    permitido: r.readable,
                })
                .collect();
        }
        self.navegador
            .entries()
            .iter()
            .filter(|e| e.kind != FsEntryKind::Other as i32)
            .map(|e| {
                let pasta = e.kind == FsEntryKind::Dir as i32;
                Item {
                    nome: e.name.clone(),
                    pasta,
                    tamanho: if pasta { 0 } else { e.size },
                    mtime_ms: e.mtime_ms,
                    permitido: true,
                }
            })
            .collect()
    }

    /// Título da folha: o caminho dentro da raiz aberta, ou vazio na lista de raízes.
    pub fn titulo(&self) -> String {
        match self.navegador.root() {
            None => String::new(),
            Some(r) => std::iter::once(r.name.as_str())
                .chain(self.navegador.dirs().iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" / "),
        }
    }

    /// Tocou no item `i` de [`Folha::itens`]. `perguntar` = rede tarifada com a confirmação
    /// ligada.
    pub fn tocar(&mut self, i: usize, perguntar: bool) -> Acao {
        let Some(item) = self.itens().into_iter().nth(i) else {
            return Acao::Nada;
        };
        if self.navegador.root().is_none() {
            let Some(id) = self.navegador.roots().get(i).map(|r| r.id) else {
                return Acao::Nada;
            };
            return self
                .navegador
                .open_root(id)
                .map_or(Acao::Nada, Acao::Listar);
        }
        if item.pasta {
            return self
                .navegador
                .open_dir(&item.nome)
                .map_or(Acao::Nada, Acao::Listar);
        }
        let Some(root) = self.navegador.root().map(|r| r.id) else {
            return Acao::Nada;
        };
        self.pedir(
            Pedido::Baixar {
                root,
                caminho: self.navegador.item_path(&item.nome),
                nome: item.nome,
                tamanho: item.tamanho,
            },
            perguntar,
        )
    }

    /// Arquivos escolhidos no seletor do sistema: vão para a pasta aberta, se dá para escrever
    /// nela, ou para a pasta de entrada do host.
    pub fn escolhidos(&mut self, arquivos: Vec<Escolhido>, perguntar: bool) -> Acao {
        if arquivos.is_empty() {
            return Acao::Nada;
        }
        let (root, base) = match self.navegador.root() {
            Some(r) if r.writable => (r.id, self.navegador.path()),
            _ => (INBOX_ROOT, String::new()),
        };
        self.pedir(
            Pedido::Enviar {
                root,
                base,
                arquivos,
            },
            perguntar,
        )
    }

    fn pedir(&mut self, p: Pedido, perguntar: bool) -> Acao {
        if perguntar && p.tamanho() > LIMITE_TARIFADA {
            // Só uma pergunta por vez: a nova ganha.
            if let Some(antigo) = self.pergunta.replace(p) {
                self.lixo.extend(antigo.temporarios());
            }
            return Acao::Perguntar;
        }
        Acao::Fazer(p)
    }

    /// A pergunta aberta, se houver.
    pub fn pergunta(&self) -> Option<&Pedido> {
        self.pergunta.as_ref()
    }

    /// Resposta à pergunta: `Some` = fazer agora. Com "não", as cópias temporárias vão para o
    /// [`Folha::lixo`].
    pub fn responder(&mut self, sim: bool) -> Option<Pedido> {
        let p = self.pergunta.take()?;
        if sim {
            Some(p)
        } else {
            self.lixo.extend(p.temporarios());
            None
        }
    }

    /// Cópias temporárias que não vão mais ser enviadas, para o app apagar.
    pub fn lixo(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.lixo)
    }

    /// Voltar: uma pasta acima; da raiz, a lista de raízes. `false` = já estava nas raízes
    /// (a folha fecha).
    pub fn voltar(&mut self) -> (bool, Option<ListRequest>) {
        if self.navegador.root().is_none() {
            return (false, None);
        }
        (true, self.navegador.up())
    }

    /// O envio `xfer_id` começou; `temporario` é apagado quando ele terminar.
    pub fn enviando(&mut self, xfer_id: u32, a: Escolhido) {
        self.transferencias.sending(xfer_id, a.nome, a.tamanho);
        self.temporarios.insert(xfer_id, a.caminho);
    }

    /// Cancelado pela pessoa. Devolve a cópia temporária para apagar.
    pub fn cancelado(&mut self, xfer_id: u32) -> Option<PathBuf> {
        self.transferencias.cancelled(xfer_id);
        self.temporarios.remove(&xfer_id)
    }

    /// Evento do serviço de arquivos da sessão.
    pub fn evento(&mut self, ev: &Event, agora: Instant) -> Efeito {
        let mut ef = Efeito::default();
        match ev {
            Event::Roots(r) => {
                self.navegador.set_roots(r);
                ef.redesenhar = true;
            }
            Event::Listing(l) => {
                ef.listar = self.navegador.apply(l);
                ef.redesenhar = true;
            }
            Event::Finished {
                xfer_id,
                direction,
                path,
            } => {
                ef.redesenhar = self.transferencias.apply(ev, agora);
                match direction {
                    Direction::In => {
                        let nome = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        ef.publicar = Some((path.clone(), nome));
                    }
                    Direction::Out => ef.apagar = self.temporarios.remove(xfer_id),
                }
            }
            Event::Failed {
                xfer_id, direction, ..
            } => {
                ef.redesenhar = self.transferencias.apply(ev, agora);
                if *direction == Direction::Out {
                    ef.apagar = self.temporarios.remove(xfer_id);
                }
            }
            Event::Receiving { .. } | Event::Progress { .. } => {
                ef.redesenhar = self.transferencias.apply(ev, agora);
            }
        }
        ef
    }

    /// Fim da sessão: as cópias temporárias que sobraram, para apagar.
    pub fn fechar(&mut self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.temporarios.drain().map(|(_, p)| p).collect();
        if let Some(p) = self.pergunta.take() {
            v.extend(p.temporarios());
        }
        v.append(&mut self.lixo);
        *self = Folha::default();
        v
    }

    /// Transferências em andamento ou esperando.
    pub fn ativas(&self) -> usize {
        self.transferencias.active()
    }

    /// Estado de uma linha para a tela: 0 esperando, 1 em andamento, 2 concluída, 3 falhou,
    /// 4 cancelada.
    pub fn estado_da_linha(s: &State) -> i32 {
        match s {
            State::Waiting => 0,
            State::Running => 1,
            State::Done { .. } => 2,
            State::Failed { .. } => 3,
            State::Cancelled => 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use taildesk_proto::control::{FsAccess, FsEntry, FsListing, FsRoot, FsRoots};

    use super::*;

    fn raizes() -> FsRoots {
        FsRoots {
            roots: vec![
                FsRoot {
                    id: 1,
                    name: "Documentos".into(),
                    access: FsAccess::ReadWrite as i32,
                },
                FsRoot {
                    id: 2,
                    name: "Fotos".into(),
                    access: FsAccess::Read as i32,
                },
                FsRoot {
                    id: 3,
                    name: "Caixa".into(),
                    access: FsAccess::Write as i32,
                },
            ],
        }
    }

    fn entrada(nome: &str, kind: FsEntryKind, size: u64) -> FsEntry {
        FsEntry {
            name: nome.into(),
            kind: kind as i32,
            size,
            mtime_ms: 0,
        }
    }

    fn escolhido(nome: &str, tamanho: u64) -> Escolhido {
        Escolhido {
            caminho: PathBuf::from("/cache/enviar").join(nome),
            nome: nome.into(),
            tamanho,
        }
    }

    /// Folha com a raiz "Documentos" aberta e uma página com uma pasta e dois arquivos.
    fn folha_aberta() -> Folha {
        let mut f = Folha::new();
        f.evento(&Event::Roots(raizes()), Instant::now());
        assert!(matches!(f.tocar(0, false), Acao::Listar(_)));
        f.navegador.requested(7);
        let ef = f.evento(
            &Event::Listing(FsListing {
                req_id: 7,
                entries: vec![
                    entrada("Projetos", FsEntryKind::Dir, 0),
                    entrada("fifo", FsEntryKind::Other, 0),
                    entrada("nota.txt", FsEntryKind::File, 12),
                    entrada("video.mkv", FsEntryKind::File, 900 * 1024 * 1024),
                ],
                more: false,
                error: 0,
            }),
            Instant::now(),
        );
        assert!(ef.redesenhar && ef.listar.is_none());
        f
    }

    #[test]
    fn raizes_e_pastas() {
        let mut f = Folha::new();
        f.evento(&Event::Roots(raizes()), Instant::now());
        let itens = f.itens();
        assert_eq!(itens.len(), 3);
        assert!(itens[1].permitido);
        // Só escrita: aparece sem abrir.
        assert!(!itens[2].permitido);
        assert_eq!(f.tocar(2, false), Acao::Nada);
        assert_eq!(f.titulo(), "");

        let mut f = folha_aberta();
        assert_eq!(f.titulo(), "Documentos");
        let nomes: Vec<String> = f.itens().into_iter().map(|i| i.nome).collect();
        assert_eq!(nomes, ["Projetos", "nota.txt", "video.mkv"]);
        match f.tocar(0, false) {
            Acao::Listar(r) => {
                assert_eq!((r.root, r.path.as_str(), r.start), (1, "Projetos", 0));
            }
            a => panic!("{a:?}"),
        }
        assert_eq!(f.titulo(), "Documentos / Projetos");
        // Voltar: da pasta para a raiz, da raiz para a lista de raízes, e aí a folha fecha.
        assert!(matches!(f.voltar(), (true, Some(_))));
        assert!(matches!(f.voltar(), (true, None)));
        assert_eq!(f.voltar(), (false, None));
    }

    #[test]
    fn baixar_e_confirmar_na_rede_tarifada() {
        let mut f = folha_aberta();
        assert_eq!(
            f.tocar(1, true),
            Acao::Fazer(Pedido::Baixar {
                root: 1,
                caminho: "nota.txt".into(),
                nome: "nota.txt".into(),
                tamanho: 12,
            })
        );
        // Grande em rede tarifada: pergunta antes.
        assert_eq!(f.tocar(2, true), Acao::Perguntar);
        assert_eq!(f.pergunta().map(|p| p.tamanho()), Some(900 * 1024 * 1024));
        assert!(matches!(f.responder(true), Some(Pedido::Baixar { .. })));
        assert!(f.pergunta().is_none());
        // Sem tarifa (ou com a confirmação desligada) vai direto.
        assert!(matches!(f.tocar(2, false), Acao::Fazer(_)));
    }

    #[test]
    fn envio_vai_para_a_pasta_aberta_ou_para_a_entrada() {
        let mut f = Folha::new();
        f.evento(&Event::Roots(raizes()), Instant::now());
        match f.escolhidos(vec![escolhido("a.jpg", 10)], false) {
            Acao::Fazer(Pedido::Enviar { root, base, .. }) => {
                assert_eq!((root, base.as_str()), (INBOX_ROOT, ""));
            }
            a => panic!("{a:?}"),
        }
        let mut f = folha_aberta();
        f.tocar(0, false);
        match f.escolhidos(vec![escolhido("a.jpg", 10)], false) {
            Acao::Fazer(Pedido::Enviar { root, base, .. }) => {
                assert_eq!((root, base.as_str()), (1, "Projetos"));
            }
            a => panic!("{a:?}"),
        }
        assert_eq!(f.escolhidos(Vec::new(), true), Acao::Nada);
    }

    #[test]
    fn temporarios_sao_apagados() {
        let mut f = Folha::new();
        let grande = || vec![escolhido("a.iso", LIMITE_TARIFADA), escolhido("b.iso", 10)];
        assert_eq!(f.escolhidos(grande(), true), Acao::Perguntar);
        assert_eq!(
            f.pergunta().map(|p| p.nome(crate::textos::Idioma::Pt)),
            Some("2 arquivos".into())
        );
        // Uma nova pergunta substitui a antiga, que vai para o lixo.
        assert_eq!(f.escolhidos(grande(), true), Acao::Perguntar);
        assert_eq!(f.lixo().len(), 2);
        assert!(f.responder(false).is_none());
        assert_eq!(f.lixo().len(), 2);
        assert!(f.lixo().is_empty());

        // Envio que termina (bem ou mal) devolve a cópia para apagar.
        f.enviando(5, escolhido("c.txt", 3));
        f.enviando(7, escolhido("d.txt", 3));
        f.enviando(9, escolhido("e.txt", 3));
        let ef = f.evento(
            &Event::Finished {
                xfer_id: 5,
                direction: Direction::Out,
                path: "/cache/enviar/c.txt".into(),
            },
            Instant::now(),
        );
        assert_eq!(ef.apagar, Some(PathBuf::from("/cache/enviar/c.txt")));
        assert!(ef.publicar.is_none());
        let ef = f.evento(
            &Event::Failed {
                xfer_id: 7,
                direction: Direction::Out,
                code: taildesk_proto::control::FileErrorCode::Denied,
                resumable: false,
            },
            Instant::now(),
        );
        assert_eq!(ef.apagar, Some(PathBuf::from("/cache/enviar/d.txt")));
        assert_eq!(f.ativas(), 1);
        // O que sobrou sai no fim da sessão.
        assert_eq!(f.fechar(), [PathBuf::from("/cache/enviar/e.txt")]);
        assert_eq!(f.ativas(), 0);
    }

    #[test]
    fn download_terminado_vai_para_a_pasta_publica() {
        let mut f = Folha::new();
        f.transferencias.downloading(3, "nota.txt");
        let ef = f.evento(
            &Event::Finished {
                xfer_id: 3,
                direction: Direction::In,
                path: "/dados/recebidos/nota (1).txt".into(),
            },
            Instant::now(),
        );
        assert_eq!(
            ef.publicar,
            Some((
                PathBuf::from("/dados/recebidos/nota (1).txt"),
                "nota (1).txt".into()
            ))
        );
        assert!(ef.redesenhar);
    }
}
