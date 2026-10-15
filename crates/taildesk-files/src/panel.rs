//! Estado do painel de arquivos da sessão (`12-levantamento-ux.md` §5.7), sem depender da UI:
//! - [`Transfers`]: a faixa de transferências, montada a partir dos [`Event`]s do serviço e das
//!   ações locais (enviar, baixar, cancelar), com velocidade e tempo restante;
//! - [`Browser`]: o "Navegar no host" (raízes, pasta atual, páginas da listagem);
//! - [`fmt`]: tamanhos, velocidade, tempo e data em pt-BR e en.
//!
//! A janela da sessão guarda um de cada, passa cada evento e lê o resultado para a tela.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use taildesk_proto::control::{FileErrorCode, FsAccess, FsEntry, FsEntryKind, FsListing, FsRoots};

use crate::audit::Direction;
use crate::service::Event;

/// Em que pé está uma transferência.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Oferta ou pedido feito; o peer ainda não aceitou (ou não há vaga).
    Waiting,
    Running,
    /// `renamed`: já havia um arquivo com o nome e o novo ganhou outro (`nome (1).txt`).
    Done {
        path: PathBuf,
        renamed: bool,
    },
    /// Com `resumable`, mandar de novo retoma de onde parou.
    Failed {
        code: FileErrorCode,
        resumable: bool,
    },
    Cancelled,
}

impl State {
    pub fn finished(&self) -> bool {
        !matches!(self, State::Waiting | State::Running)
    }
}

/// Uma linha da faixa.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub xfer_id: u32,
    /// Nome do arquivo como o usuário o conhece (o original, mesmo se renomeado no destino).
    pub name: String,
    pub direction: Direction,
    /// 0 enquanto o download não começou.
    pub size: u64,
    pub done: u64,
    pub state: State,
    /// Bytes por segundo, média móvel; 0 sem medida.
    pub rate: f64,
    last: Option<(Instant, u64)>,
}

impl Row {
    /// 0.0 a 1.0.
    pub fn fraction(&self) -> f32 {
        match self.state {
            State::Done { .. } => 1.0,
            _ if self.size == 0 => 0.0,
            _ => (self.done as f64 / self.size as f64).min(1.0) as f32,
        }
    }

    /// Tempo restante pela velocidade média; `None` sem medida ou fora de andamento.
    pub fn eta(&self) -> Option<Duration> {
        if self.state != State::Running || self.rate < 1.0 || self.size == 0 {
            return None;
        }
        let left = self.size.saturating_sub(self.done) as f64;
        Some(Duration::from_secs_f64(left / self.rate))
    }
}

/// Peso de cada medida nova na velocidade média (≈ últimos 4 avisos).
const RATE_ALPHA: f64 = 0.25;
/// Avisos mais próximos que isto entram juntos no próximo (evita velocidade com dt ≈ 0).
const RATE_MIN_DT: Duration = Duration::from_millis(200);

/// A faixa de transferências de uma sessão, mais recentes por último.
#[derive(Debug, Default)]
pub struct Transfers {
    rows: Vec<Row>,
}

impl Transfers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn get(&self, xfer_id: u32) -> Option<&Row> {
        self.rows.iter().find(|r| r.xfer_id == xfer_id)
    }

    /// Quantas ainda não terminaram.
    pub fn active(&self) -> usize {
        self.rows.iter().filter(|r| !r.state.finished()).count()
    }

    fn row(&mut self, xfer_id: u32) -> Option<&mut Row> {
        self.rows.iter_mut().find(|r| r.xfer_id == xfer_id)
    }

    fn push(&mut self, xfer_id: u32, name: String, direction: Direction, size: u64) {
        if let Some(r) = self.row(xfer_id) {
            r.name = name;
            r.size = size;
            return;
        }
        self.rows.push(Row {
            xfer_id,
            name,
            direction,
            size,
            done: 0,
            state: State::Waiting,
            rate: 0.0,
            last: None,
        });
    }

    /// Depois de `Files::send_file` devolver o id.
    pub fn sending(&mut self, xfer_id: u32, name: impl Into<String>, size: u64) {
        self.push(xfer_id, name.into(), Direction::Out, size);
    }

    /// Depois de `Files::download`; o tamanho chega com o `Event::Receiving`.
    pub fn downloading(&mut self, xfer_id: u32, name: impl Into<String>) {
        self.push(xfer_id, name.into(), Direction::In, 0);
    }

    /// Depois de `Files::cancel` (o serviço não manda evento para o cancelamento local).
    pub fn cancelled(&mut self, xfer_id: u32) {
        if let Some(r) = self.row(xfer_id)
            && !r.state.finished()
        {
            r.state = State::Cancelled;
            r.rate = 0.0;
        }
    }

    /// Tira as terminadas com sucesso ou canceladas; as falhas ficam para retomar.
    pub fn clear_finished(&mut self) {
        self.rows
            .retain(|r| !matches!(r.state, State::Done { .. } | State::Cancelled));
    }

    /// Aplica um evento do serviço. Devolve `true` se alguma linha mudou.
    pub fn apply(&mut self, ev: &Event, now: Instant) -> bool {
        match ev {
            Event::Receiving {
                xfer_id,
                name,
                size,
            } => {
                // Download nosso: mantém o nome pedido; envio do peer: linha nova.
                let short = name.rsplit('/').next().unwrap_or(name).to_owned();
                match self.row(*xfer_id) {
                    Some(r) => {
                        r.size = *size;
                        r.state = State::Running;
                    }
                    None => {
                        self.push(*xfer_id, short, Direction::In, *size);
                        if let Some(r) = self.row(*xfer_id) {
                            r.state = State::Running;
                        }
                    }
                }
                true
            }
            Event::Progress {
                xfer_id,
                done,
                total,
            } => {
                let Some(r) = self.row(*xfer_id) else {
                    return false;
                };
                if r.state.finished() {
                    return false;
                }
                r.state = State::Running;
                r.size = *total;
                match r.last {
                    // Primeiro aviso (pode vir de uma retomada): só marca o ponto de partida.
                    None => r.last = Some((now, *done)),
                    Some((t, d)) => {
                        let dt = now.saturating_duration_since(t);
                        if dt >= RATE_MIN_DT && *done >= d {
                            let sample = (*done - d) as f64 / dt.as_secs_f64();
                            r.rate = if r.rate == 0.0 {
                                sample
                            } else {
                                r.rate + RATE_ALPHA * (sample - r.rate)
                            };
                            r.last = Some((now, *done));
                        } else if *done < d {
                            // Recomeçou do zero (`.part` divergente).
                            r.last = Some((now, *done));
                        }
                    }
                }
                r.done = *done;
                true
            }
            Event::Finished {
                xfer_id,
                direction,
                path,
            } => {
                let Some(r) = self.row(*xfer_id) else {
                    return false;
                };
                let renamed = *direction == Direction::In
                    && path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy() != r.name);
                r.done = r.size;
                r.rate = 0.0;
                r.state = State::Done {
                    path: path.clone(),
                    renamed,
                };
                true
            }
            Event::Failed {
                xfer_id,
                code,
                resumable,
                ..
            } => {
                let Some(r) = self.row(*xfer_id) else {
                    return false;
                };
                if r.state.finished() {
                    return false;
                }
                r.rate = 0.0;
                r.state = if *code == FileErrorCode::Cancelled {
                    State::Cancelled
                } else {
                    State::Failed {
                        code: *code,
                        resumable: *resumable,
                    }
                };
                true
            }
            Event::Roots(_) | Event::Listing(_) => false,
        }
    }
}

/// Uma página a pedir com `Files::list(root, &path, start)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRequest {
    pub root: u32,
    pub path: String,
    pub start: u32,
}

/// Uma pasta que o host libera.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub id: u32,
    pub name: String,
    /// Sem leitura: aparece como "Sem permissão" em vez de sumir.
    pub readable: bool,
    pub writable: bool,
}

/// O "Navegar no host".
#[derive(Debug, Default)]
pub struct Browser {
    roots: Vec<Root>,
    root: Option<u32>,
    /// Pastas abertas a partir da raiz.
    dirs: Vec<String>,
    entries: Vec<FsEntry>,
    /// `req_id` da página esperada; respostas de pastas antigas são ignoradas.
    pending: Option<u32>,
    error: Option<FileErrorCode>,
}

impl Browser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn roots(&self) -> &[Root] {
        &self.roots
    }

    pub fn root(&self) -> Option<&Root> {
        self.root
            .and_then(|id| self.roots.iter().find(|r| r.id == id))
    }

    /// Pastas, depois arquivos, na ordem do host.
    pub fn entries(&self) -> &[FsEntry] {
        &self.entries
    }

    pub fn loading(&self) -> bool {
        self.pending.is_some()
    }

    pub fn error(&self) -> Option<FileErrorCode> {
        self.error
    }

    /// Caminho da pasta atual dentro da raiz, com `/` (`""` = a raiz).
    pub fn path(&self) -> String {
        self.dirs.join("/")
    }

    /// Partes do caminho, para o "migalhas de pão".
    pub fn dirs(&self) -> &[String] {
        &self.dirs
    }

    /// Caminho de um item da pasta atual, para `Files::download`.
    pub fn item_path(&self, name: &str) -> String {
        if self.dirs.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", self.path())
        }
    }

    /// Resposta ao `Files::request_roots`. Se a raiz aberta sumiu, volta à lista de raízes.
    pub fn set_roots(&mut self, roots: &FsRoots) {
        self.roots = roots
            .roots
            .iter()
            .map(|r| {
                let access = FsAccess::try_from(r.access).unwrap_or(FsAccess::Unspecified);
                Root {
                    id: r.id,
                    name: r.name.clone(),
                    readable: matches!(access, FsAccess::Read | FsAccess::ReadWrite),
                    writable: matches!(access, FsAccess::Write | FsAccess::ReadWrite),
                }
            })
            .collect();
        if self.root().is_none() {
            self.close_root();
        }
    }

    /// Volta à lista de raízes.
    pub fn close_root(&mut self) {
        self.root = None;
        self.dirs.clear();
        self.entries.clear();
        self.pending = None;
        self.error = None;
    }

    fn reload(&mut self) -> Option<ListRequest> {
        let root = self.root?;
        self.entries.clear();
        self.error = None;
        Some(ListRequest {
            root,
            path: self.path(),
            start: 0,
        })
    }

    /// Abre uma raiz. `None` se ela não existe ou não pode ser lida.
    pub fn open_root(&mut self, id: u32) -> Option<ListRequest> {
        if !self.roots.iter().any(|r| r.id == id && r.readable) {
            return None;
        }
        self.root = Some(id);
        self.dirs.clear();
        self.reload()
    }

    /// Entra numa pasta da lista atual.
    pub fn open_dir(&mut self, name: &str) -> Option<ListRequest> {
        let is_dir = self
            .entries
            .iter()
            .any(|e| e.name == name && e.kind == FsEntryKind::Dir as i32);
        if !is_dir {
            return None;
        }
        self.dirs.push(name.to_owned());
        self.reload()
    }

    /// Sobe até `depth` pastas a partir da raiz (0 = a raiz).
    pub fn go_to(&mut self, depth: usize) -> Option<ListRequest> {
        self.root?;
        self.dirs.truncate(depth);
        self.reload()
    }

    /// Uma pasta acima; da raiz, volta à lista de raízes.
    pub fn up(&mut self) -> Option<ListRequest> {
        if self.dirs.pop().is_some() {
            self.reload()
        } else {
            self.close_root();
            None
        }
    }

    /// Lista a pasta atual de novo.
    pub fn refresh(&mut self) -> Option<ListRequest> {
        self.reload()
    }

    /// Guarda o `req_id` que `Files::list` devolveu para o pedido.
    pub fn requested(&mut self, req_id: u32) {
        self.pending = Some(req_id);
    }

    /// Aplica uma página. Devolve o pedido da próxima, se houver. Páginas que não são a esperada
    /// são ignoradas.
    pub fn apply(&mut self, listing: &FsListing) -> Option<ListRequest> {
        if self.pending != Some(listing.req_id) {
            return None;
        }
        self.pending = None;
        if listing.error != 0 {
            self.error =
                Some(FileErrorCode::try_from(listing.error).unwrap_or(FileErrorCode::Unspecified));
            return None;
        }
        self.entries.extend(listing.entries.iter().cloned());
        if !listing.more {
            return None;
        }
        Some(ListRequest {
            root: self.root?,
            path: self.path(),
            start: u32::try_from(self.entries.len()).unwrap_or(u32::MAX),
        })
    }
}

/// Textos com número (o resto dos textos fica no Slint, com `@tr`).
pub mod fmt {
    use std::time::Duration;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub enum Locale {
        #[default]
        PtBr,
        En,
    }

    fn decimal(v: f64, locale: Locale) -> String {
        let s = if v < 10.0 {
            format!("{v:.1}")
        } else {
            format!("{v:.0}")
        };
        match locale {
            Locale::PtBr => s.replace('.', ","),
            Locale::En => s,
        }
    }

    /// Tamanho em unidades de 1024, como os gerenciadores de arquivos: `1,5 GB`, `820 KB`.
    pub fn size(bytes: u64, locale: Locale) -> String {
        const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
        if bytes < 1024 {
            return format!("{bytes} B");
        }
        let mut v = bytes as f64 / 1024.0;
        let mut unit = 0;
        while v >= 1023.95 && unit < UNITS.len() - 1 {
            v /= 1024.0;
            unit += 1;
        }
        format!("{} {}", decimal(v, locale), UNITS[unit])
    }

    /// Velocidade: `12 MB/s`.
    pub fn rate(bytes_per_s: f64, locale: Locale) -> String {
        format!("{}/s", size(bytes_per_s.max(0.0) as u64, locale))
    }

    /// Tempo restante curto: `45 s`, `3 min 20 s`, `1 h 05 min`.
    pub fn eta(d: Duration) -> String {
        let s = d.as_secs();
        if s < 60 {
            format!("{s} s")
        } else if s < 3600 {
            format!("{} min {:02} s", s / 60, s % 60)
        } else {
            format!("{} h {:02} min", s / 3600, (s % 3600) / 60)
        }
    }

    /// Data de modificação (UTC, só o dia): `15/10/2026` ou `2026-10-15`. Vazio se desconhecida.
    pub fn date(mtime_ms: i64, locale: Locale) -> String {
        if mtime_ms <= 0 {
            return String::new();
        }
        let (y, m, d) = civil_from_days(mtime_ms.div_euclid(86_400_000));
        match locale {
            Locale::PtBr => format!("{d:02}/{m:02}/{y}"),
            Locale::En => format!("{y}-{m:02}-{d:02}"),
        }
    }

    /// Dias desde 1970-01-01 para (ano, mês, dia), calendário gregoriano proléptico
    /// (algoritmo de Howard Hinnant, domínio público).
    fn civil_from_days(z: i64) -> (i64, u32, u32) {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (if m <= 2 { y + 1 } else { y }, m, d)
    }
}

#[cfg(test)]
mod tests {
    use super::fmt::{Locale, date, eta, rate, size};
    use super::*;
    use taildesk_proto::control::FsRoot;

    fn entry(name: &str, kind: FsEntryKind) -> FsEntry {
        FsEntry {
            name: name.into(),
            kind: kind as i32,
            size: 10,
            mtime_ms: 0,
        }
    }

    #[test]
    fn upload_progress_rate_and_done() {
        let t0 = Instant::now();
        let mut t = Transfers::new();
        t.sending(1, "video.mkv", 100 << 20);
        assert_eq!(t.rows()[0].state, State::Waiting);
        assert_eq!(t.active(), 1);

        let p = |done| Event::Progress {
            xfer_id: 1,
            done,
            total: 100 << 20,
        };
        assert!(t.apply(&p(1 << 20), t0));
        assert_eq!(t.rows()[0].rate, 0.0);
        assert!(t.apply(&p(11 << 20), t0 + Duration::from_secs(1)));
        let r = &t.rows()[0];
        assert_eq!(r.state, State::Running);
        assert!((r.rate - (10 << 20) as f64).abs() < 1.0);
        assert_eq!(r.eta(), Some(Duration::from_secs_f64(8.9)));
        assert!((r.fraction() - 0.11).abs() < 1e-6);

        // Aviso colado no anterior não derruba a média.
        t.apply(&p(12 << 20), t0 + Duration::from_millis(1010));
        assert!((t.rows()[0].rate - (10 << 20) as f64).abs() < 1.0);

        assert!(t.apply(
            &Event::Finished {
                xfer_id: 1,
                direction: Direction::Out,
                path: "/home/y/video.mkv".into(),
            },
            t0 + Duration::from_secs(10),
        ));
        let r = &t.rows()[0];
        assert!(matches!(r.state, State::Done { renamed: false, .. }));
        assert_eq!(r.fraction(), 1.0);
        assert_eq!(r.eta(), None);
        assert_eq!(t.active(), 0);
        // Progresso atrasado depois do fim é ignorado.
        assert!(!t.apply(&p(50 << 20), t0 + Duration::from_secs(11)));
        t.clear_finished();
        assert!(t.rows().is_empty());
    }

    #[test]
    fn download_gets_size_and_detects_rename() {
        let t0 = Instant::now();
        let mut t = Transfers::new();
        t.downloading(3, "relatório.pdf");
        assert_eq!(t.rows()[0].size, 0);
        assert_eq!(t.rows()[0].fraction(), 0.0);
        t.apply(
            &Event::Receiving {
                xfer_id: 3,
                name: "relatório.pdf".into(),
                size: 4096,
            },
            t0,
        );
        assert_eq!(t.rows()[0].size, 4096);
        assert_eq!(t.rows()[0].state, State::Running);
        t.apply(
            &Event::Finished {
                xfer_id: 3,
                direction: Direction::In,
                path: PathBuf::from("Downloads").join("relatório (1).pdf"),
            },
            t0,
        );
        assert!(matches!(
            t.rows()[0].state,
            State::Done { renamed: true, .. }
        ));
    }

    #[test]
    fn peer_upload_creates_row_with_short_name() {
        let mut t = Transfers::new();
        t.apply(
            &Event::Receiving {
                xfer_id: 2,
                name: "fotos/2026/praia.jpg".into(),
                size: 9,
            },
            Instant::now(),
        );
        let r = &t.rows()[0];
        assert_eq!(r.name, "praia.jpg");
        assert_eq!(r.direction, Direction::In);
        assert_eq!(r.state, State::Running);
    }

    #[test]
    fn failures_and_cancel() {
        let now = Instant::now();
        let mut t = Transfers::new();
        t.sending(1, "a", 10);
        t.sending(3, "b", 10);
        t.sending(5, "c", 10);
        let fail = |xfer_id, code, resumable| Event::Failed {
            xfer_id,
            direction: Direction::Out,
            code,
            resumable,
        };
        t.apply(&fail(1, FileErrorCode::Io, true), now);
        t.apply(&fail(3, FileErrorCode::Cancelled, true), now);
        t.cancelled(5);
        assert_eq!(
            t.rows()[0].state,
            State::Failed {
                code: FileErrorCode::Io,
                resumable: true
            }
        );
        assert_eq!(t.rows()[1].state, State::Cancelled);
        assert_eq!(t.rows()[2].state, State::Cancelled);
        // O `Failed(Cancelled)` que o peer devolve não sobrescreve.
        assert!(!t.apply(&fail(5, FileErrorCode::Io, false), now));
        // Evento de id desconhecido não cria linha.
        assert!(!t.apply(&fail(99, FileErrorCode::Io, false), now));
        t.clear_finished();
        assert_eq!(t.rows().len(), 1);
        assert_eq!(t.rows()[0].xfer_id, 1);
    }

    #[test]
    fn restart_from_zero_does_not_break_rate() {
        let t0 = Instant::now();
        let mut t = Transfers::new();
        t.sending(1, "a", 100);
        let p = |done| Event::Progress {
            xfer_id: 1,
            done,
            total: 100,
        };
        t.apply(&p(60), t0);
        t.apply(&p(0), t0 + Duration::from_secs(1));
        t.apply(&p(20), t0 + Duration::from_secs(2));
        assert!((t.rows()[0].rate - 20.0).abs() < 1e-9);
    }

    fn roots() -> FsRoots {
        FsRoots {
            roots: vec![
                FsRoot {
                    id: 1,
                    name: "Documentos".into(),
                    access: FsAccess::ReadWrite.into(),
                },
                FsRoot {
                    id: 2,
                    name: "Caixa".into(),
                    access: FsAccess::Write.into(),
                },
            ],
        }
    }

    #[test]
    fn browse_pages_dirs_and_up() {
        let mut b = Browser::new();
        b.set_roots(&roots());
        assert!(b.roots()[0].readable && b.roots()[0].writable);
        assert!(!b.roots()[1].readable);
        assert_eq!(b.open_root(2), None);
        assert_eq!(b.open_root(9), None);

        let req = b.open_root(1).unwrap();
        assert_eq!(
            req,
            ListRequest {
                root: 1,
                path: String::new(),
                start: 0
            }
        );
        b.requested(11);
        assert!(b.loading());
        let next = b
            .apply(&FsListing {
                req_id: 11,
                entries: vec![entry("projetos", FsEntryKind::Dir)],
                more: true,
                error: 0,
            })
            .unwrap();
        assert_eq!(next.start, 1);
        b.requested(13);
        assert_eq!(
            b.apply(&FsListing {
                req_id: 13,
                entries: vec![entry("leia.txt", FsEntryKind::File)],
                more: false,
                error: 0,
            }),
            None
        );
        assert_eq!(b.entries().len(), 2);
        assert!(!b.loading());

        assert_eq!(b.open_dir("leia.txt"), None);
        let req = b.open_dir("projetos").unwrap();
        assert_eq!(req.path, "projetos");
        assert!(b.entries().is_empty());
        b.requested(15);
        // Resposta atrasada da pasta anterior é ignorada.
        assert_eq!(
            b.apply(&FsListing {
                req_id: 13,
                entries: vec![entry("velho", FsEntryKind::File)],
                more: false,
                error: 0,
            }),
            None
        );
        assert!(b.entries().is_empty());
        b.apply(&FsListing {
            req_id: 15,
            entries: vec![entry("sub", FsEntryKind::Dir)],
            more: false,
            error: 0,
        });
        b.open_dir("sub").unwrap();
        assert_eq!(b.path(), "projetos/sub");
        assert_eq!(b.item_path("x.bin"), "projetos/sub/x.bin");

        assert_eq!(b.go_to(1).unwrap().path, "projetos");
        assert_eq!(b.up().unwrap().path, "");
        assert_eq!(b.item_path("x.bin"), "x.bin");
        assert_eq!(b.up(), None);
        assert!(b.root().is_none());
    }

    #[test]
    fn listing_error_and_root_removed() {
        let mut b = Browser::new();
        b.set_roots(&roots());
        b.open_root(1).unwrap();
        b.requested(7);
        b.apply(&FsListing {
            req_id: 7,
            entries: vec![],
            more: false,
            error: FileErrorCode::Denied as i32,
        });
        assert_eq!(b.error(), Some(FileErrorCode::Denied));
        assert_eq!(b.refresh().unwrap().path, "");
        assert_eq!(b.error(), None);

        b.set_roots(&FsRoots { roots: vec![] });
        assert!(b.root().is_none());
        assert_eq!(b.refresh(), None);
    }

    #[test]
    fn formats() {
        assert_eq!(size(0, Locale::PtBr), "0 B");
        assert_eq!(size(1023, Locale::PtBr), "1023 B");
        assert_eq!(size(1536, Locale::PtBr), "1,5 KB");
        assert_eq!(size(1536, Locale::En), "1.5 KB");
        assert_eq!(size(820 * 1024, Locale::PtBr), "820 KB");
        assert_eq!(size(1024 * 1024 - 1, Locale::PtBr), "1,0 MB");
        assert_eq!(size(10 << 30, Locale::PtBr), "10 GB");
        assert_eq!(size(u64::MAX, Locale::En), "16384 PB");
        assert_eq!(rate(12.0 * 1048576.0, Locale::PtBr), "12 MB/s");
        assert_eq!(rate(-1.0, Locale::PtBr), "0 B/s");
        assert_eq!(eta(Duration::from_secs(45)), "45 s");
        assert_eq!(eta(Duration::from_secs(200)), "3 min 20 s");
        assert_eq!(eta(Duration::from_secs(3900)), "1 h 05 min");
        assert_eq!(date(0, Locale::PtBr), "");
        assert_eq!(date(1_792_022_400_000, Locale::PtBr), "15/10/2026");
        assert_eq!(date(1_792_022_400_000, Locale::En), "2026-10-15");
        assert_eq!(date(951_782_400_000, Locale::En), "2000-02-29");
    }
}
