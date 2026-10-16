//! Área de transferência compartilhada: liga os backends de `taildesk_platform` à
//! [`Sistema`] da sessão.
//!
//! - Host no Linux: portal `Clipboard` da sessão RemoteDesktop ([`portal`]).
//! - Controlador no Wayland: `wl_data_device` da janela da sessão ([`janela_wayland`]).
//!
//! Arquivos copiados ainda não passam (só texto, HTML e imagem PNG).

use std::sync::{Arc, Mutex, PoisonError};

use taildesk_proto::control::{ClipErrorCode, ClipFormat};
use taildesk_session::clipboard::Copiado;
use taildesk_session::conexao::clipboard::DoSistema;
use tokio::sync::{mpsc, oneshot};

/// Fila do sistema para a ponte.
const FILA: usize = 16;

/// Formatos que passam por enquanto.
fn suportado(f: ClipFormat) -> bool {
    matches!(f, ClipFormat::Text | ClipFormat::Html | ClipFormat::Png)
}

/// O que se sabe dos dois lados: os MIME do que foi copiado aqui e a oferta do peer em vigor.
#[derive(Debug, Default)]
struct Estado {
    locais: Vec<String>,
    oferta: Option<(u32, Vec<ClipFormat>)>,
}

#[derive(Clone, Debug, Default)]
struct Compartilhado(Arc<Mutex<Estado>>);

impl Compartilhado {
    fn com<R>(&self, f: impl FnOnce(&mut Estado) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Alguém copiou aqui com estes MIME: o que anunciar ao peer.
    fn copiou(&self, mimes: Vec<String>) -> Copiado {
        let formatos = taildesk_platform::area::formatos(&mimes)
            .into_iter()
            .filter(|f| suportado(*f))
            .map(|f| (f, 0))
            .collect();
        self.com(|e| e.locais = mimes);
        Copiado {
            formatos,
            arquivos: Vec::new(),
        }
    }

    /// Guarda a oferta do peer; devolve os MIME a anunciar no sistema.
    fn oferecer(&self, seq: u32, c: &Copiado) -> Vec<String> {
        let formatos: Vec<ClipFormat> = c
            .formatos
            .iter()
            .map(|(f, _)| *f)
            .filter(|f| suportado(*f))
            .collect();
        let mimes = formatos
            .iter()
            .flat_map(|&f| {
                taildesk_platform::area::mimes(f)
                    .iter()
                    .map(|m| m.to_string())
            })
            .collect();
        self.com(|e| e.oferta = Some((seq, formatos)));
        mimes
    }

    /// Um app daqui colou `mime` da oferta do peer: `seq` e formato.
    fn colar(&self, mime: &str) -> Option<(u32, ClipFormat)> {
        let f = taildesk_platform::area::formato_do_mime(mime)?;
        self.com(|e| {
            e.oferta
                .as_ref()
                .filter(|(_, fs)| fs.contains(&f))
                .map(|(seq, _)| (*seq, f))
        })
    }

    fn mime_para_ler(&self, f: ClipFormat) -> Option<String> {
        self.com(|e| taildesk_platform::area::mime_para_ler(f, &e.locais).map(str::to_owned))
    }
}

/// Bytes lidos do sistema, com o texto normalizado.
fn lido(f: ClipFormat, bytes: Vec<u8>) -> Vec<u8> {
    match f {
        ClipFormat::Text | ClipFormat::Html => taildesk_platform::area::texto_normalizado(&bytes),
        _ => bytes,
    }
}

/// Pede à ponte o conteúdo que um app daqui colou. `None` = cole vazio.
fn pedir(
    sistema: &mpsc::Sender<DoSistema>,
    seq: u32,
    formato: ClipFormat,
) -> Option<oneshot::Receiver<Option<Vec<u8>>>> {
    let (resposta, rx) = oneshot::channel();
    sistema
        .try_send(DoSistema::Colar {
            seq,
            formato,
            resposta,
        })
        .ok()?;
    Some(rx)
}

#[cfg(target_os = "linux")]
pub use linux::{AreaPortal, janela_wayland, portal};

#[cfg(target_os = "linux")]
mod linux {
    use std::future::Future;
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::time::Duration;

    use taildesk_platform::area_wayland::{AreaWayland, DaArea, ler_tudo};
    use taildesk_platform::wayland::SessaoPortal;
    use taildesk_proto::control::limits::CLIP_TOTAL;
    use taildesk_session::conexao::clipboard::Sistema;

    use super::*;

    /// Prazo para o app que copiou no host escrever o conteúdo.
    const LEITURA_MAX: Duration = Duration::from_secs(5);

    fn escrever(fd: OwnedFd, dados: &[u8]) -> bool {
        std::fs::File::from(fd).write_all(dados).is_ok()
    }

    /// Área de transferência do host pelo portal; soltar para as tarefas.
    #[derive(Debug)]
    pub struct AreaPortal {
        portal: Arc<SessaoPortal>,
        estado: Compartilhado,
        tarefas: Vec<tokio::task::JoinHandle<()>>,
    }

    impl Drop for AreaPortal {
        fn drop(&mut self) {
            for t in &self.tarefas {
                t.abort();
            }
        }
    }

    /// A área de transferência da sessão do portal, se ela tem (`None` em portais sem
    /// `Clipboard` ou se a pessoa não liberou).
    pub fn portal(portal: Arc<SessaoPortal>) -> Option<(AreaPortal, mpsc::Receiver<DoSistema>)> {
        if !portal.tem_area() {
            return None;
        }
        let (tx, rx) = mpsc::channel(FILA);
        let estado = Compartilhado::default();
        let mudou = tokio::spawn({
            let (portal, estado, tx) = (portal.clone(), estado.clone(), tx.clone());
            async move {
                use futures_util::StreamExt;
                let Ok(s) = portal.area_mudou().await else {
                    return;
                };
                let mut s = std::pin::pin!(s);
                while let Some(mimes) = s.next().await {
                    if tx
                        .send(DoSistema::Copiou(estado.copiou(mimes)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
        let pedidos = tokio::spawn({
            let (portal, estado) = (portal.clone(), estado.clone());
            async move {
                use futures_util::StreamExt;
                let Ok(s) = portal.area_pedidos().await else {
                    return;
                };
                let mut s = std::pin::pin!(s);
                while let Some((mime, serial)) = s.next().await {
                    let resposta = estado.colar(&mime).and_then(|(seq, f)| pedir(&tx, seq, f));
                    let portal = portal.clone();
                    // Um pedido não segura os outros (o peer pode demorar).
                    tokio::spawn(async move {
                        let dados = match resposta {
                            Some(rx) => rx.await.ok().flatten(),
                            None => None,
                        };
                        let ok = match (dados, portal.area_escrever(serial).await) {
                            (Some(d), Ok(fd)) => {
                                tokio::task::spawn_blocking(move || escrever(fd, &d))
                                    .await
                                    .unwrap_or(false)
                            }
                            _ => false,
                        };
                        let _ = portal.area_escrita_feita(serial, ok).await;
                    });
                }
            }
        });
        Some((
            AreaPortal {
                portal,
                estado,
                tarefas: vec![mudou, pedidos],
            },
            rx,
        ))
    }

    impl Sistema for AreaPortal {
        fn oferecer(&mut self, seq: u32, copiado: &Copiado) {
            let mimes = self.estado.oferecer(seq, copiado);
            let portal = self.portal.clone();
            tokio::spawn(async move {
                let m: Vec<&str> = mimes.iter().map(String::as_str).collect();
                if let Err(e) = portal.area_oferecer(&m).await {
                    eprintln!("taildesk: não foi possível pôr a cópia do controlador no host: {e}");
                }
            });
        }

        fn ler(
            &mut self,
            formato: ClipFormat,
        ) -> impl Future<Output = Result<Vec<u8>, ClipErrorCode>> + Send + 'static {
            let mime = self.estado.mime_para_ler(formato);
            let portal = self.portal.clone();
            async move {
                let mime = mime.ok_or(ClipErrorCode::Unavailable)?;
                let fd = portal
                    .area_ler(&mime)
                    .await
                    .map_err(|_| ClipErrorCode::Unavailable)?;
                let bytes = tokio::task::spawn_blocking(move || {
                    ler_tudo(fd, CLIP_TOTAL as usize, LEITURA_MAX)
                })
                .await
                .map_err(|_| ClipErrorCode::Unavailable)?
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::InvalidData {
                        ClipErrorCode::TooBig
                    } else {
                        ClipErrorCode::Unavailable
                    }
                })?;
                Ok(lido(formato, bytes))
            }
        }
    }

    /// Área de transferência da janela da sessão no controlador (Wayland).
    #[derive(Debug)]
    pub struct AreaJanela {
        area: Arc<AreaWayland>,
        estado: Compartilhado,
    }

    /// Liga a área de transferência à janela.
    ///
    /// # Safety
    ///
    /// `display` tem de ser o `wl_display` vivo da janela e viver mais que a área.
    pub unsafe fn janela_wayland(
        display: std::ptr::NonNull<std::ffi::c_void>,
    ) -> std::io::Result<(AreaJanela, mpsc::Receiver<DoSistema>)> {
        let (tx, rx) = mpsc::channel(FILA);
        let estado = Compartilhado::default();
        let e = estado.clone();
        let aviso = move |a: DaArea| match a {
            DaArea::Copiou(mimes) => {
                let _ = tx.try_send(DoSistema::Copiou(e.copiou(mimes)));
            }
            DaArea::Colar { mime, fd } => {
                let resposta = e.colar(&mime).and_then(|(seq, f)| pedir(&tx, seq, f));
                // Esperar o peer fora da thread do Wayland; fechar o fd sem escrever = vazio.
                std::thread::spawn(move || {
                    if let Some(d) = resposta.and_then(|rx| rx.blocking_recv().ok().flatten()) {
                        escrever(fd, &d);
                    }
                });
            }
        };
        // SAFETY: repassado do contrato desta função.
        let area = unsafe { AreaWayland::abrir(display, aviso)? };
        Ok((
            AreaJanela {
                area: Arc::new(area),
                estado,
            },
            rx,
        ))
    }

    impl Sistema for AreaJanela {
        fn oferecer(&mut self, seq: u32, copiado: &Copiado) {
            let mimes = self.estado.oferecer(seq, copiado);
            let _ = self.area.oferecer(mimes);
        }

        fn ler(
            &mut self,
            formato: ClipFormat,
        ) -> impl Future<Output = Result<Vec<u8>, ClipErrorCode>> + Send + 'static {
            let mime = self.estado.mime_para_ler(formato);
            let area = self.area.clone();
            async move {
                let mime = mime.ok_or(ClipErrorCode::Unavailable)?;
                let bytes =
                    tokio::task::spawn_blocking(move || area.ler(&mime, CLIP_TOTAL as usize))
                        .await
                        .map_err(|_| ClipErrorCode::Unavailable)?
                        .map_err(|_| ClipErrorCode::Unavailable)?;
                Ok(lido(formato, bytes))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copiar_e_oferecer_por_mime() {
        let e = Compartilhado::default();
        let c = e.copiou(vec![
            "text/uri-list".into(),
            "text/plain;charset=utf-8".into(),
            "image/png".into(),
        ]);
        // Arquivos ainda não passam.
        assert_eq!(
            c.formatos,
            vec![(ClipFormat::Text, 0), (ClipFormat::Png, 0)]
        );
        assert_eq!(
            e.mime_para_ler(ClipFormat::Text).as_deref(),
            Some("text/plain;charset=utf-8")
        );

        let mimes = e.oferecer(
            4,
            &Copiado {
                formatos: vec![(ClipFormat::Html, 10), (ClipFormat::Files, 0)],
                arquivos: Vec::new(),
            },
        );
        assert_eq!(mimes, vec!["text/html".to_string()]);
        assert_eq!(e.colar("text/html"), Some((4, ClipFormat::Html)));
        assert_eq!(e.colar("text/plain"), None);
    }
}
