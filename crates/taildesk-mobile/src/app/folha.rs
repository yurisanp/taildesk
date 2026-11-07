//! Liga a folha de arquivos ([`crate::arquivos::Folha`]) às telas e ao serviço de arquivos da
//! sessão (`SessaoRede::arquivos`). O pedido vai para o runtime da rede; a resposta volta pelos
//! eventos da sessão ([`App::evento_arquivos`]) ou por [`no_app`].

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ModelRc, SharedString, VecModel};
use taildesk_files::audit::Direction;
use taildesk_files::panel::fmt::{self, Locale};
use taildesk_files::panel::{ListRequest, State};
use taildesk_files::service::{Event, Files};
use taildesk_proto::control::FileErrorCode;
use taildesk_proto::files::INBOX_ROOT;

use super::{Alvo, App, Tela, no_app};
use crate::arquivos::{Acao, Escolhido, Pedido};
use crate::textos::Idioma;
use crate::ui::{ItemArquivo, LinhaArquivo};

/// Progresso redesenha no máximo a cada tanto; mudanças de estado redesenham na hora.
const INTERVALO_PROGRESSO: Duration = Duration::from_millis(150);

/// Nome da pasta pública dos downloads, para os avisos.
const PASTA_PUBLICA: &str = "Download/TailDesk";

/// Apaga cópias temporárias de envios (fora da thread da interface não precisa: são poucas).
pub(super) fn apagar(caminhos: Vec<PathBuf>) {
    for c in caminhos {
        let _ = std::fs::remove_file(&c);
        // A casca Kotlin põe cada escolha numa pasta própria: some junto se ficou vazia.
        if let Some(p) = c.parent() {
            let _ = std::fs::remove_dir(p);
        }
    }
}

/// Lê `caminho\tnome\ttamanho` por linha (o que a casca Kotlin copiou do seletor).
pub fn escolhidos_de(texto: &str) -> Vec<Escolhido> {
    texto
        .lines()
        .filter_map(|l| {
            let mut c = l.split('\t');
            let caminho = PathBuf::from(c.next().filter(|s| !s.is_empty())?);
            // O nome vira o último trecho do caminho no host: sem barras.
            let nome = c
                .next()
                .map(|n| n.replace(['/', '\\'], "_"))
                .filter(|n| !n.is_empty() && n != "." && n != "..")
                .or_else(|| {
                    caminho
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })?;
            let tamanho = c
                .next()
                .and_then(|t| t.parse().ok())
                .or_else(|| std::fs::metadata(&caminho).ok().map(|m| m.len()))
                .unwrap_or(0);
            Some(Escolhido {
                caminho,
                nome,
                tamanho,
            })
        })
        .collect()
}

fn texto_do_erro(idioma: Idioma, code: FileErrorCode) -> &'static str {
    match code {
        FileErrorCode::Denied => idioma.t(
            "O computador não permite arquivos nesta sessão ou nesta pasta.",
            "The computer does not allow files in this session or folder.",
        ),
        FileErrorCode::NotFound => idioma.t("Não encontrado.", "Not found."),
        FileErrorCode::BadPath => idioma.t("Caminho não aceito.", "Path not accepted."),
        FileErrorCode::NoSpace => idioma.t("Sem espaço no destino.", "No space left."),
        FileErrorCode::HashMismatch => idioma.t(
            "O arquivo chegou diferente do original.",
            "The file arrived corrupted.",
        ),
        FileErrorCode::Busy => idioma.t(
            "Muitas transferências ao mesmo tempo.",
            "Too many transfers at once.",
        ),
        FileErrorCode::Changed => idioma.t(
            "O arquivo mudou durante a cópia.",
            "The file changed during the copy.",
        ),
        FileErrorCode::Cancelled => idioma.t("Cancelado.", "Cancelled."),
        FileErrorCode::Io | FileErrorCode::ResumeMismatch | FileErrorCode::Protocol => {
            idioma.t("A conexão falhou.", "The connection failed.")
        }
        FileErrorCode::Unspecified => idioma.t("Falhou.", "Failed."),
    }
}

impl App {
    fn servico_arquivos(&self) -> Option<Files<quinn::Connection>> {
        match &self.estado.borrow().alvo {
            Some(Alvo::Rede(Some(s))) => Some(s.arquivos()),
            _ => None,
        }
    }

    /// Pergunta antes dos arquivos grandes: rede tarifada com "Economizar em dados móveis".
    fn perguntar_arquivo(&self) -> bool {
        let e = self.estado.borrow();
        e.overlay.sinais().tarifada && e.config.economizar_em_movel
    }

    pub(super) fn abrir_arquivos(self: &Rc<Self>) {
        let Some(servico) = self.servico_arquivos() else {
            if let Some(ui) = self.ui() {
                ui.set_arquivos_aberto(false);
            }
            self.toast(self.t(
                "Os arquivos funcionam numa sessão com um computador de verdade.",
                "Files work in a session with a real computer.",
            ));
            return;
        };
        // Pede as pastas toda vez: o host pode ter mudado o que libera.
        self.estado.borrow().rt.spawn(async move {
            servico.request_roots().await;
        });
        self.desenhar_arquivos();
    }

    fn pedir_lista(&self, req: ListRequest) {
        let Some(servico) = self.servico_arquivos() else {
            return;
        };
        let geracao = self.estado.borrow().geracao;
        self.estado.borrow().rt.spawn(async move {
            let req_id = servico.list(req.root, &req.path, req.start).await;
            no_app(move |a| {
                let mut e = a.estado.borrow_mut();
                if e.geracao == geracao {
                    e.arquivos.navegador.requested(req_id);
                }
                drop(e);
                a.desenhar_arquivos();
            });
        });
    }

    fn tratar(&self, acao: Acao) {
        match acao {
            Acao::Nada | Acao::Perguntar => {}
            Acao::Listar(r) => self.pedir_lista(r),
            Acao::Fazer(p) => self.fazer(p),
        }
        let lixo = self.estado.borrow_mut().arquivos.lixo();
        apagar(lixo);
        self.desenhar_arquivos();
    }

    pub(super) fn arquivo_tocado(&self, i: i32) {
        let perguntar = self.perguntar_arquivo();
        let Ok(i) = usize::try_from(i) else { return };
        let acao = self.estado.borrow_mut().arquivos.tocar(i, perguntar);
        self.tratar(acao);
    }

    pub(super) fn arquivos_voltar(&self) {
        let (fica, req) = self.estado.borrow_mut().arquivos.voltar();
        if !fica && let Some(ui) = self.ui() {
            ui.set_arquivos_aberto(false);
        }
        if let Some(r) = req {
            self.pedir_lista(r);
        }
        self.desenhar_arquivos();
    }

    pub(super) fn responder_arquivo(&self, sim: bool) {
        let p = self.estado.borrow_mut().arquivos.responder(sim);
        self.tratar(p.map_or(Acao::Nada, Acao::Fazer));
    }

    pub(super) fn cancelar_transferencia(&self, id: i32) {
        let xfer = u32::try_from(id).unwrap_or(0);
        if let Some(s) = self.servico_arquivos() {
            self.estado
                .borrow()
                .rt
                .spawn(async move { s.cancel(xfer).await });
        }
        let temp = self.estado.borrow_mut().arquivos.cancelado(xfer);
        apagar(temp.into_iter().collect());
        self.desenhar_arquivos();
    }

    /// A casca Kotlin copiou os arquivos escolhidos no seletor (`caminho\tnome\ttamanho` por
    /// linha; vazio = a pessoa desistiu).
    pub fn arquivos_escolhidos(&self, texto: &str) {
        let escolhidos = escolhidos_de(texto);
        if self.servico_arquivos().is_none() {
            // A sessão acabou enquanto o seletor estava aberto.
            apagar(escolhidos.into_iter().map(|a| a.caminho).collect());
            return;
        }
        let perguntar = self.perguntar_arquivo();
        let acao = self
            .estado
            .borrow_mut()
            .arquivos
            .escolhidos(escolhidos, perguntar);
        self.tratar(acao);
    }

    /// A casca Kotlin terminou de copiar um download para `Download/TailDesk` (`ok\tnome` ou
    /// `erro\tnome`).
    pub fn download_publicado(&self, texto: &str) {
        let (ok, nome) = texto.split_once('\t').unwrap_or((texto, ""));
        let t = if ok == "ok" {
            format!("{nome} {} {PASTA_PUBLICA}.", self.t("salvo em", "saved to"))
        } else {
            format!(
                "{} {nome} {} {PASTA_PUBLICA}.",
                self.t("Não foi possível salvar", "Could not save"),
                self.t("em", "to")
            )
        };
        self.toast(t);
    }

    /// Onde os downloads chegam antes de irem para a pasta pública.
    fn pasta_recebidos(&self) -> PathBuf {
        let base = self
            .estado
            .borrow()
            .plat
            .pasta_dados()
            .unwrap_or_else(std::env::temp_dir);
        base.join("recebidos")
    }

    fn fazer(&self, p: Pedido) {
        let Some(servico) = self.servico_arquivos() else {
            apagar(p.temporarios());
            return;
        };
        let geracao = self.estado.borrow().geracao;
        match p {
            Pedido::Baixar {
                root,
                caminho,
                nome,
                ..
            } => {
                let destino = self.pasta_recebidos();
                if let Err(e) = std::fs::create_dir_all(&destino) {
                    crate::registrar!("taildesk: pasta de recebidos: {e}");
                }
                self.estado.borrow().rt.spawn(async move {
                    let xfer = servico.download(root, &caminho, &destino).await;
                    no_app(move |a| {
                        if a.estado.borrow().geracao != geracao {
                            return;
                        }
                        a.estado
                            .borrow_mut()
                            .arquivos
                            .transferencias
                            .downloading(xfer, nome);
                        a.desenhar_arquivos();
                    });
                });
            }
            Pedido::Enviar {
                root,
                base,
                arquivos,
            } => {
                self.estado.borrow().rt.spawn(async move {
                    for a in arquivos {
                        let rel = if base.is_empty() {
                            a.nome.clone()
                        } else {
                            format!("{base}/{}", a.nome)
                        };
                        // Espera uma vaga (até 4 envios ao mesmo tempo) antes de virar linha.
                        let r = servico.send_file(&a.caminho, root, &rel).await;
                        no_app(move |app| match r {
                            Ok(xfer) if app.estado.borrow().geracao == geracao => {
                                app.estado.borrow_mut().arquivos.enviando(xfer, a);
                                app.desenhar_arquivos();
                            }
                            Ok(_) => apagar(vec![a.caminho]),
                            Err(e) => {
                                crate::registrar!("taildesk: enviar {}: {e}", a.nome);
                                let t = format!(
                                    "{} {}.",
                                    app.t("Não foi possível enviar", "Could not send"),
                                    a.nome
                                );
                                apagar(vec![a.caminho]);
                                app.toast(t);
                                app.envio_um(false);
                            }
                        });
                    }
                });
            }
        }
    }

    /// Evento do serviço de arquivos da sessão.
    pub(super) fn evento_arquivos(self: &Rc<Self>, ev: &Event) {
        let agora = Instant::now();
        let ef = self.estado.borrow_mut().arquivos.evento(ev, agora);
        match ev {
            Event::Finished {
                direction: Direction::Out,
                ..
            } => self.envio_um(true),
            Event::Failed {
                direction: Direction::Out,
                ..
            } => self.envio_um(false),
            _ => {}
        }
        if let Some(r) = ef.listar {
            self.pedir_lista(r);
        }
        if let Some(c) = ef.apagar {
            apagar(vec![c]);
        }
        if let Some((caminho, nome)) = ef.publicar {
            let publicou = self.estado.borrow().plat.publicar_download(&caminho, &nome);
            if !publicou {
                let pasta = caminho
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                self.toast(format!(
                    "{nome} {} {pasta}.",
                    self.t("salvo em", "saved to")
                ));
            }
        }
        if let Event::Failed {
            direction: Direction::Out,
            code: FileErrorCode::Denied,
            ..
        } = ev
        {
            self.toast(texto_do_erro(
                self.estado.borrow().idioma,
                FileErrorCode::Denied,
            ));
        }
        if !ef.redesenhar {
            return;
        }
        let progresso = matches!(ev, Event::Progress { .. });
        let cedo = self
            .estado
            .borrow()
            .ultimo_desenho_arquivos
            .is_some_and(|t| agora.duration_since(t) < INTERVALO_PROGRESSO);
        if !(progresso && cedo) {
            self.desenhar_arquivos();
        }
    }

    /// Arquivos do menu de compartilhar, já copiados para o cache (`caminho\tnome\ttamanho`
    /// por linha). Numa sessão aberta vão direto para ela; fora, a pessoa escolhe o computador.
    pub fn compartilhar(self: &Rc<Self>, texto: &str) {
        let escolhidos = escolhidos_de(texto);
        if escolhidos.is_empty() {
            return;
        }
        let em_sessao = self.ui().is_some_and(|u| u.get_tela() == Tela::Sessao)
            && self.servico_arquivos().is_some()
            && self.estado.borrow().envio.is_none();
        if em_sessao {
            if let Some(ui) = self.ui() {
                ui.set_arquivos_aberto(true);
            }
            self.arquivos_escolhidos(texto);
            return;
        }
        let antigos = std::mem::replace(&mut self.estado.borrow_mut().compartilhar, escolhidos);
        apagar(antigos.into_iter().map(|a| a.caminho).collect());
        if let Some(ui) = self.ui()
            && matches!(ui.get_tela(), Tela::Dispositivos | Tela::Configuracoes)
        {
            ui.set_tela(Tela::Dispositivos);
        }
        self.mostrar_compartilhar();
    }

    pub(super) fn cancelar_compartilhar(&self) {
        let antigos = std::mem::take(&mut self.estado.borrow_mut().compartilhar);
        apagar(antigos.into_iter().map(|a| a.caminho).collect());
        self.mostrar_compartilhar();
    }

    /// A faixa "Escolha o computador…" da lista de dispositivos.
    pub(super) fn mostrar_compartilhar(&self) {
        let Some(ui) = self.ui() else { return };
        let e = self.estado.borrow();
        let texto = if e.compartilhar.is_empty() {
            String::new()
        } else {
            crate::compartilhar::faixa(&e.compartilhar, e.overlay.sinais().tarifada, e.idioma)
        };
        drop(e);
        ui.set_compartilhando(texto.into());
    }

    /// A sessão de envio ficou ao vivo: oferece os arquivos para a pasta de entrada.
    pub(super) fn enviar_compartilhados(&self) {
        let arquivos = match self.estado.borrow_mut().envio.as_mut() {
            Some(e) => e.comecar(),
            None => return,
        };
        self.fazer(Pedido::Enviar {
            root: INBOX_ROOT,
            base: String::new(),
            arquivos,
        });
    }

    /// Um arquivo do envio sem sessão terminou; no último, avisa e encerra.
    fn envio_um(&self, ok: bool) {
        let resumo = match self.estado.borrow_mut().envio.as_mut() {
            Some(e) if e.comecou() => e.terminou(ok),
            _ => None,
        };
        let Some(r) = resumo else { return };
        let nome = self
            .estado
            .borrow()
            .host
            .as_ref()
            .map(|h| h.nome.clone())
            .unwrap_or_default();
        let texto = r.texto(self.estado.borrow().idioma, &nome);
        let geracao = self.estado.borrow().geracao;
        // Na próxima volta do laço, fora do tratamento do evento.
        no_app(move |a| {
            if a.estado.borrow().geracao == geracao {
                a.encerrar();
                a.toast(texto);
            }
        });
    }

    pub(super) fn desenhar_arquivos(&self) {
        let Some(ui) = self.ui() else { return };
        let mut e = self.estado.borrow_mut();
        e.ultimo_desenho_arquivos = Some(Instant::now());
        let idioma = e.idioma;
        let l = match idioma {
            Idioma::Pt => Locale::PtBr,
            Idioma::En => Locale::En,
        };
        let f = &e.arquivos;
        let itens: Vec<ItemArquivo> = f
            .itens()
            .into_iter()
            .map(|i| ItemArquivo {
                nome: i.nome.into(),
                detalhe: if i.pasta {
                    SharedString::new()
                } else {
                    let data = fmt::date(i.mtime_ms, l);
                    let tam = fmt::size(i.tamanho, l);
                    if data.is_empty() {
                        tam.into()
                    } else {
                        format!("{tam} · {data}").into()
                    }
                },
                pasta: i.pasta,
                permitido: i.permitido,
            })
            .collect();
        let linhas: Vec<LinhaArquivo> = f
            .transferencias
            .rows()
            .iter()
            .rev()
            .map(|r| {
                let detalhe = match &r.state {
                    State::Waiting => idioma.t("Esperando…", "Waiting…").to_owned(),
                    State::Running => {
                        let mut t = format!(
                            "{} {} {}",
                            fmt::size(r.done, l),
                            idioma.t("de", "of"),
                            fmt::size(r.size, l)
                        );
                        if r.rate > 0.0 {
                            t.push_str(" · ");
                            t.push_str(&fmt::rate(r.rate, l));
                        }
                        t
                    }
                    State::Done { .. } if r.direction == Direction::In => {
                        format!("{} {PASTA_PUBLICA}", idioma.t("Salvo em", "Saved to"))
                    }
                    State::Done { .. } => idioma.t("Enviado", "Sent").to_owned(),
                    State::Failed { code, .. } => texto_do_erro(idioma, *code).to_owned(),
                    State::Cancelled => idioma.t("Cancelado", "Cancelled").to_owned(),
                };
                LinhaArquivo {
                    id: i32::try_from(r.xfer_id).unwrap_or(i32::MAX),
                    nome: r.name.as_str().into(),
                    enviando: r.direction == Direction::Out,
                    estado: crate::arquivos::Folha::estado_da_linha(&r.state),
                    progresso: r.fraction(),
                    detalhe: detalhe.into(),
                }
            })
            .collect();
        let pergunta = f.pergunta().map_or_else(String::new, |p| {
            format!(
                "{} ({}) {}",
                p.nome(idioma),
                fmt::size(p.tamanho(), l),
                idioma.t(
                    "é grande e você está numa rede tarifada.",
                    "is large and you are on a metered network."
                )
            )
        });
        ui.set_arquivos_titulo(f.titulo().into());
        ui.set_arquivos_carregando(f.navegador.loading());
        ui.set_arquivos_erro(
            f.navegador
                .error()
                .map_or("", |c| texto_do_erro(idioma, c))
                .into(),
        );
        ui.set_arquivos_pergunta(pergunta.into());
        drop(e);
        ui.set_arquivos_itens(ModelRc::new(VecModel::from(itens)));
        ui.set_arquivos_transferencias(ModelRc::new(VecModel::from(linhas)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_os_escolhidos() {
        let v = escolhidos_de(
            "/cache/enviar/1/foto.jpg\tfoto.jpg\t2048\n\
             /cache/enviar/1/x\ta/b.txt\t5\n\
             \n\
             /cache/enviar/1/..\t..\t1\n",
        );
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].nome, "foto.jpg");
        assert_eq!(v[0].tamanho, 2048);
        // Barra no nome não vira pasta no host.
        assert_eq!(v[1].nome, "a_b.txt");
        assert!(escolhidos_de("").is_empty());
    }
}
