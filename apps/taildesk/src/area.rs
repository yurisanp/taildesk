//! Área de transferência compartilhada: liga os backends de `taildesk_platform` à
//! [`Sistema`] da sessão.
//!
//! - Host no Linux: portal `Clipboard` da sessão RemoteDesktop ([`portal`]).
//! - Controlador no Wayland: `wl_data_device` da janela da sessão ([`janela_wayland`]).
//! - Windows (host e controlador): a área de transferência do Win32 (`windows`).
//!
//! Arquivos copiados vão pelo serviço de arquivos da sessão: aqui só se lê a lista do que foi
//! copiado ([`arquivos_locais`]) e se entrega ao sistema a lista do que foi baixado
//! ([`para_o_sistema`]).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use taildesk_proto::control::limits::CLIP_FILES;
use taildesk_proto::control::{ClipErrorCode, ClipFile, ClipFormat};
use taildesk_session::clipboard::Copiado;
use taildesk_session::conexao::clipboard::DoSistema;
use tokio::sync::{mpsc, oneshot};

/// Fila do sistema para a ponte.
const FILA: usize = 16;

/// Formatos que passam.
fn suportado(f: ClipFormat) -> bool {
    matches!(
        f,
        ClipFormat::Text | ClipFormat::Html | ClipFormat::Png | ClipFormat::Files
    )
}

/// Os arquivos e pastas copiados aqui que dá para oferecer (os que existem e têm nome em
/// UTF-8), com a lista para a oferta e os caminhos para o serviço de arquivos.
fn arquivos_locais(caminhos: Vec<PathBuf>) -> (Vec<ClipFile>, Vec<PathBuf>) {
    let mut lista = Vec::new();
    let mut locais = Vec::new();
    for c in caminhos.into_iter().take(CLIP_FILES) {
        let (Ok(meta), Some(nome)) = (
            std::fs::metadata(&c),
            c.file_name().and_then(|n| n.to_str()).map(str::to_owned),
        ) else {
            continue;
        };
        lista.push(ClipFile {
            name: nome,
            size: if meta.is_dir() { 0 } else { meta.len() },
            dir: meta.is_dir(),
        });
        locais.push(c);
    }
    (lista, locais)
}

/// Põe em `c` os arquivos copiados; sem nenhum, tira o formato.
fn com_arquivos(mut c: Copiado, caminhos: Vec<PathBuf>) -> Copiado {
    (c.arquivos, c.locais) = arquivos_locais(caminhos);
    if c.arquivos.is_empty() {
        c.formatos.retain(|(f, _)| *f != ClipFormat::Files);
    }
    c
}

/// O que um app daqui recebe ao colar `mime`: arquivos viram a lista de URIs que ele lê; o resto
/// vai como veio do peer.
#[cfg_attr(windows, allow(dead_code))]
fn para_o_sistema(mime: &str, dados: Vec<u8>) -> Vec<u8> {
    #[cfg(unix)]
    if taildesk_platform::area::formato_do_mime(mime) == Some(ClipFormat::Files) {
        let caminhos: Vec<PathBuf> = String::from_utf8_lossy(&dados)
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect();
        return taildesk_platform::area::caminhos_para_uris(mime, &caminhos);
    }
    let _ = mime;
    dados
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
    #[cfg_attr(windows, allow(dead_code))]
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
            locais: Vec::new(),
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
    #[cfg_attr(windows, allow(dead_code))]
    fn colar(&self, mime: &str) -> Option<(u32, ClipFormat)> {
        let f = taildesk_platform::area::formato_do_mime(mime)?;
        self.com(|e| {
            e.oferta
                .as_ref()
                .filter(|(_, fs)| fs.contains(&f))
                .map(|(seq, _)| (*seq, f))
        })
    }

    /// `seq` da oferta do peer em vigor, se ela tem `f`.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn seq_da_oferta(&self, f: ClipFormat) -> Option<u32> {
        self.com(|e| {
            e.oferta
                .as_ref()
                .filter(|(_, fs)| fs.contains(&f))
                .map(|(seq, _)| *seq)
        })
    }

    #[cfg_attr(windows, allow(dead_code))]
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

#[cfg(windows)]
pub use win::{AreaWin, windows};

#[cfg(windows)]
mod win {
    use std::future::Future;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use taildesk_platform::area_windows::{AreaWindows, DaArea};
    use taildesk_session::conexao::clipboard::Sistema;

    use super::*;

    /// Quanto um app que colou espera o peer (o Windows segura o app nesse tempo).
    const COLAR_MAX: Duration = Duration::from_secs(10);
    /// Arquivos são baixados antes de o Explorer receber a lista.
    const COLAR_ARQUIVOS_MAX: Duration = Duration::from_secs(600);

    /// Área de transferência do Windows ligada à sessão.
    #[derive(Debug)]
    pub struct AreaWin {
        area: Arc<AreaWindows>,
        estado: Compartilhado,
    }

    /// Abre a área de transferência do Windows (a mesma no host e no controlador).
    pub fn windows() -> std::io::Result<(AreaWin, mpsc::Receiver<DoSistema>)> {
        let (tx, rx) = mpsc::channel(FILA);
        let estado = Compartilhado::default();
        let aviso = {
            let tx = tx.clone();
            move |a: DaArea| match a {
                DaArea::Copiou(formatos, caminhos) => {
                    let formatos = formatos
                        .into_iter()
                        .filter(|f| suportado(*f))
                        .map(|f| (f, 0))
                        .collect();
                    let c = Copiado {
                        formatos,
                        ..Copiado::default()
                    };
                    let _ = tx.try_send(DoSistema::Copiou(com_arquivos(c, caminhos)));
                }
            }
        };
        let pedir_peer = {
            let estado = estado.clone();
            move |f: ClipFormat| {
                let seq = estado.seq_da_oferta(f)?;
                let mut rx = pedir(&tx, seq, f)?;
                // Na thread da área (o app que colou espera): sem runtime, com prazo.
                let fim = Instant::now()
                    + if f == ClipFormat::Files {
                        COLAR_ARQUIVOS_MAX
                    } else {
                        COLAR_MAX
                    };
                loop {
                    match rx.try_recv() {
                        Ok(d) => return d,
                        Err(oneshot::error::TryRecvError::Closed) => return None,
                        Err(oneshot::error::TryRecvError::Empty) if Instant::now() < fim => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(oneshot::error::TryRecvError::Empty) => return None,
                    }
                }
            }
        };
        let area = AreaWindows::abrir(aviso, pedir_peer)?;
        Ok((
            AreaWin {
                area: Arc::new(area),
                estado,
            },
            rx,
        ))
    }

    impl Sistema for AreaWin {
        fn oferecer(&mut self, seq: u32, copiado: &Copiado) {
            self.estado.oferecer(seq, copiado);
            let formatos = copiado
                .formatos
                .iter()
                .map(|(f, _)| *f)
                .filter(|f| suportado(*f))
                .collect();
            let _ = self.area.oferecer(formatos);
        }

        fn ler(
            &mut self,
            formato: ClipFormat,
        ) -> impl Future<Output = Result<Vec<u8>, ClipErrorCode>> + Send + 'static {
            let area = self.area.clone();
            async move {
                let bytes = tokio::task::spawn_blocking(move || area.ler(formato))
                    .await
                    .map_err(|_| ClipErrorCode::Unavailable)?
                    .map_err(|e| {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            ClipErrorCode::Gone
                        } else {
                            ClipErrorCode::Unavailable
                        }
                    })?;
                if bytes.len() as u64 > taildesk_proto::control::limits::CLIP_TOTAL {
                    return Err(ClipErrorCode::TooBig);
                }
                Ok(lido(formato, bytes))
            }
        }
    }
}

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

    /// Lê `mime` do que foi copiado no host.
    async fn ler_portal(portal: &SessaoPortal, mime: &str) -> Result<Vec<u8>, ClipErrorCode> {
        let fd = portal
            .area_ler(mime)
            .await
            .map_err(|_| ClipErrorCode::Unavailable)?;
        tokio::task::spawn_blocking(move || ler_tudo(fd, CLIP_TOTAL as usize, LEITURA_MAX))
            .await
            .map_err(|_| ClipErrorCode::Unavailable)?
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::InvalidData {
                    ClipErrorCode::TooBig
                } else {
                    ClipErrorCode::Unavailable
                }
            })
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
                    let mut c = estado.copiou(mimes);
                    if c.formatos.iter().any(|(f, _)| *f == ClipFormat::Files) {
                        let caminhos = match estado.mime_para_ler(ClipFormat::Files) {
                            Some(m) => ler_portal(&portal, &m).await.unwrap_or_default(),
                            None => Vec::new(),
                        };
                        c = com_arquivos(c, taildesk_platform::area::uris_para_caminhos(&caminhos));
                    }
                    if tx.send(DoSistema::Copiou(c)).await.is_err() {
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
                                let d = para_o_sistema(&mime, d);
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
                Ok(lido(formato, ler_portal(&portal, &mime).await?))
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
        // A lista de arquivos copiados é lida da própria área, fora da thread do Wayland.
        let propria: Arc<std::sync::OnceLock<Arc<AreaWayland>>> = Arc::default();
        let p = propria.clone();
        let aviso = move |a: DaArea| match a {
            DaArea::Copiou(mimes) => {
                let c = e.copiou(mimes);
                let ler = c
                    .formatos
                    .iter()
                    .any(|(f, _)| *f == ClipFormat::Files)
                    .then(|| e.mime_para_ler(ClipFormat::Files))
                    .flatten();
                match (ler, p.get().cloned()) {
                    (Some(mime), Some(area)) => {
                        let tx = tx.clone();
                        std::thread::spawn(move || {
                            let uris = area.ler(&mime, CLIP_TOTAL as usize).unwrap_or_default();
                            let caminhos = taildesk_platform::area::uris_para_caminhos(&uris);
                            let _ = tx.try_send(DoSistema::Copiou(com_arquivos(c, caminhos)));
                        });
                    }
                    _ => {
                        let _ = tx.try_send(DoSistema::Copiou(com_arquivos(c, Vec::new())));
                    }
                }
            }
            DaArea::Colar { mime, fd } => {
                let resposta = e.colar(&mime).and_then(|(seq, f)| pedir(&tx, seq, f));
                // Esperar o peer fora da thread do Wayland; fechar o fd sem escrever = vazio.
                std::thread::spawn(move || {
                    if let Some(d) = resposta.and_then(|rx| rx.blocking_recv().ok().flatten()) {
                        escrever(fd, &para_o_sistema(&mime, d));
                    }
                });
            }
        };
        // SAFETY: repassado do contrato desta função.
        let area = Arc::new(unsafe { AreaWayland::abrir(display, aviso)? });
        let _ = propria.set(area.clone());
        Ok((AreaJanela { area, estado }, rx))
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
    fn arquivos_copiados_e_colados() {
        let dir = std::env::temp_dir().join(format!("taildesk-area-app-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pasta")).unwrap();
        std::fs::write(dir.join("a.txt"), b"abc").unwrap();
        let (lista, locais) = arquivos_locais(vec![
            dir.join("a.txt"),
            dir.join("pasta"),
            dir.join("sumiu"),
        ]);
        assert_eq!(locais, vec![dir.join("a.txt"), dir.join("pasta")]);
        assert_eq!(
            lista,
            vec![
                ClipFile {
                    name: "a.txt".into(),
                    size: 3,
                    dir: false
                },
                ClipFile {
                    name: "pasta".into(),
                    size: 0,
                    dir: true
                }
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
        #[cfg(unix)]
        assert_eq!(
            para_o_sistema("text/uri-list", b"/tmp/x y\n/tmp/z".to_vec()),
            b"file:///tmp/x%20y\r\nfile:///tmp/z\r\n"
        );
        assert_eq!(para_o_sistema("text/html", b"<b>".to_vec()), b"<b>");
    }

    #[test]
    fn copiar_e_oferecer_por_mime() {
        let e = Compartilhado::default();
        let c = e.copiou(vec![
            "text/uri-list".into(),
            "text/plain;charset=utf-8".into(),
            "image/png".into(),
        ]);
        assert_eq!(
            c.formatos,
            vec![
                (ClipFormat::Text, 0),
                (ClipFormat::Png, 0),
                (ClipFormat::Files, 0)
            ]
        );
        // Sem nenhum arquivo que exista, o formato sai.
        let c = com_arquivos(c, vec![PathBuf::from("/nao/existe")]);
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
                formatos: vec![(ClipFormat::Html, 10), (ClipFormat::Png, 0)],
                arquivos: Vec::new(),
                locais: Vec::new(),
            },
        );
        assert_eq!(
            mimes,
            vec!["text/html".to_string(), "image/png".to_string()]
        );
        assert_eq!(e.colar("text/html"), Some((4, ClipFormat::Html)));
        assert_eq!(e.colar("text/plain"), None);
    }
}
