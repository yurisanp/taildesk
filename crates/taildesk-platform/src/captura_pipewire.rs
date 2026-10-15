//! Recebe os quadros de cada monitor pelo PipeWire (Linux).
//!
//! Os nodes vêm da [`crate::wayland::SessaoPortal`]: um por monitor liberado, todos na conexão de
//! [`crate::wayland::SessaoPortal::abrir_pipewire`]. Uma thread com o laço do PipeWire atende os
//! streams de todos os monitores; o callback recebe cada quadro emprestado direto do buffer
//! mapeado, sem alocar nem copiar. Quem consome copia para o próprio buffer (o encoder) e devolve
//! rápido, porque o compositor espera o buffer de volta.
//!
//! Formatos pedidos: BGRx, RGBx, BGRA, RGBA em memória compartilhada (memfd), o que todo portal
//! entrega. DMA-BUF sem cópia entra com o encode por hardware (fase 4). Monitor que ninguém está
//! vendo é pausado (`ativar(id, false)`): o compositor para de produzir quadros para ele
//! (`13-multimonitor.md` §4.2).

use std::cell::RefCell;
use std::fmt;
use std::io;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::thread::JoinHandle;

use pipewire as pw;
use pw::spa;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::Pod;

/// Ordem dos bytes de cada pixel na memória.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatoPixel {
    /// B, G, R, (ignorado). O mais comum no KDE e no GNOME.
    Bgrx,
    Rgbx,
    Bgra,
    Rgba,
}

impl FormatoPixel {
    fn de(f: VideoFormat) -> Option<FormatoPixel> {
        Some(match f {
            VideoFormat::BGRx => FormatoPixel::Bgrx,
            VideoFormat::RGBx => FormatoPixel::Rgbx,
            VideoFormat::BGRA => FormatoPixel::Bgra,
            VideoFormat::RGBA => FormatoPixel::Rgba,
            _ => return None,
        })
    }
}

/// Um quadro emprestado do buffer do PipeWire; vale só dentro do callback.
#[derive(Debug)]
pub struct Quadro<'a> {
    pub display_id: u32,
    pub largura: u32,
    pub altura: u32,
    /// Bytes por linha (pode ser maior que `largura * 4`).
    pub stride: usize,
    pub formato: FormatoPixel,
    /// `altura` linhas de `stride` bytes; a última pode ter só `largura * 4`.
    pub dados: &'a [u8],
}

impl Quadro<'_> {
    /// Uma linha da imagem, só com os pixels visíveis.
    pub fn linha(&self, y: u32) -> &[u8] {
        let ini = y as usize * self.stride;
        &self.dados[ini..ini + self.largura as usize * 4]
    }
}

/// Como chegar ao PipeWire.
#[derive(Debug)]
pub enum Conexao {
    /// Conexão restrita da sessão do portal (`SessaoPortal::abrir_pipewire`).
    Portal(OwnedFd),
    /// Socket padrão do usuário (testes e ferramentas de diagnóstico).
    Padrao,
}

enum Comando {
    Ativar(u32, bool),
    Parar,
}

/// Captura em andamento. Soltar para a captura e espera a thread do PipeWire terminar.
pub struct CapturaPipeWire {
    tx: pw::channel::Sender<Comando>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for CapturaPipeWire {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturaPipeWire").finish_non_exhaustive()
    }
}

impl CapturaPipeWire {
    /// Conecta um stream por monitor (`(display_id, node_id)`) e chama `ao_quadro` a cada
    /// quadro novo, na thread do PipeWire.
    pub fn iniciar<F>(conexao: Conexao, monitores: &[(u32, u32)], ao_quadro: F) -> io::Result<Self>
    where
        F: FnMut(&Quadro<'_>) + Send + 'static,
    {
        let (tx, rx) = pw::channel::channel::<Comando>();
        let (pronto_tx, pronto_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        let monitores = monitores.to_vec();
        let thread = std::thread::Builder::new()
            .name("taildesk-pipewire".into())
            .spawn(move || {
                if let Err(e) = laco(conexao, &monitores, ao_quadro, rx, &pronto_tx) {
                    let _ = pronto_tx.send(Err(e.to_string()));
                }
            })?;
        match pronto_rx.recv() {
            Ok(Ok(())) => Ok(CapturaPipeWire {
                tx,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(io::Error::other(e))
            }
            Err(_) => {
                let _ = thread.join();
                Err(io::Error::other(
                    "a thread do PipeWire terminou antes de começar",
                ))
            }
        }
    }

    /// Pausa ou retoma um monitor. Pausado, o compositor não produz quadros para ele.
    pub fn ativar(&self, display_id: u32, ativo: bool) {
        let _ = self.tx.send(Comando::Ativar(display_id, ativo));
    }
}

impl Drop for CapturaPipeWire {
    fn drop(&mut self) {
        let _ = self.tx.send(Comando::Parar);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Estado {
    display_id: u32,
    formato: Option<(FormatoPixel, u32, u32)>,
}

fn laco<F>(
    conexao: Conexao,
    monitores: &[(u32, u32)],
    ao_quadro: F,
    rx: pw::channel::Receiver<Comando>,
    pronto: &std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), pw::Error>
where
    F: FnMut(&Quadro<'_>) + 'static,
{
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = match conexao {
        Conexao::Portal(fd) => context.connect_fd_rc(fd, None)?,
        Conexao::Padrao => context.connect_rc(None)?,
    };
    let ao_quadro = Rc::new(RefCell::new(ao_quadro));

    let mut streams = Vec::with_capacity(monitores.len());
    let mut ouvintes = Vec::with_capacity(monitores.len());
    for &(display_id, node_id) in monitores {
        let stream = pw::stream::StreamRc::new(
            core.clone(),
            &format!("taildesk-monitor-{display_id}"),
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )?;
        let cb = Rc::clone(&ao_quadro);
        let ouvinte = stream
            .add_local_listener_with_user_data(Estado {
                display_id,
                formato: None,
            })
            .param_changed(|_, estado, id, param| {
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Some(param) = param else {
                    estado.formato = None;
                    return;
                };
                let mut info = VideoInfoRaw::default();
                estado.formato = info.parse(param).ok().and_then(|_| {
                    let tam = info.size();
                    FormatoPixel::de(info.format()).map(|f| (f, tam.width, tam.height))
                });
            })
            .process(move |stream, estado| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let Some((formato, largura, altura)) = estado.formato else {
                    return;
                };
                let Some(dado) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let chunk = dado.chunk();
                let (offset, tamanho, stride) = (
                    chunk.offset() as usize,
                    chunk.size() as usize,
                    chunk.stride(),
                );
                // Tamanho 0 = o compositor não mudou nada (só o cursor, por exemplo).
                if tamanho == 0 || stride <= 0 {
                    return;
                }
                let Some(mem) = dado.data() else {
                    return;
                };
                let Some(dados) = mem.get(offset..offset + tamanho) else {
                    return;
                };
                let quadro = Quadro {
                    display_id: estado.display_id,
                    largura,
                    altura,
                    stride: stride as usize,
                    formato,
                    dados,
                };
                if quadro_valido(&quadro) {
                    (cb.borrow_mut())(&quadro);
                }
            })
            .register()?;

        let params = formatos_aceitos();
        let mut params = [Pod::from_bytes(&params).expect("pod serializado por nós")];
        stream.connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )?;
        streams.push((display_id, stream));
        ouvintes.push(ouvinte);
    }

    let laco_fraco = main_loop.downgrade();
    let streams = Rc::new(streams);
    let streams_cmd = Rc::clone(&streams);
    let _rx = rx.attach(main_loop.loop_(), move |cmd| match cmd {
        Comando::Ativar(id, ativo) => {
            if let Some((_, s)) = streams_cmd.iter().find(|(d, _)| *d == id) {
                let _ = s.set_active(ativo);
            }
        }
        Comando::Parar => {
            if let Some(l) = laco_fraco.upgrade() {
                l.quit();
            }
        }
    });

    let _ = pronto.send(Ok(()));
    main_loop.run();
    for (_, s) in streams.iter() {
        let _ = s.disconnect();
    }
    drop(ouvintes);
    Ok(())
}

/// O buffer cobre `altura` linhas de `stride` bytes (a última pode ser curta).
fn quadro_valido(q: &Quadro<'_>) -> bool {
    let linha = q.largura as usize * 4;
    q.largura > 0
        && q.altura > 0
        && q.stride >= linha
        && q.dados.len() >= (q.altura as usize - 1) * q.stride + linha
}

/// `EnumFormat`: vídeo cru em 4 bytes por pixel, qualquer tamanho e taxa.
fn formatos_aceitos() -> Vec<u8> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::RGBx,
            VideoFormat::BGRA,
            VideoFormat::RGBA
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 60, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 240, denom: 1 }
        ),
    );
    spa::pod::serialize::PodSerializer::serialize(
        io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .expect("serializar o EnumFormat")
    .0
    .into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadro_confere_stride_e_tamanho() {
        let dados = vec![0u8; 16 * 3 + 8];
        let q = Quadro {
            display_id: 0,
            largura: 2,
            altura: 4,
            stride: 16,
            formato: FormatoPixel::Bgrx,
            dados: &dados,
        };
        assert!(quadro_valido(&q));
        assert_eq!(q.linha(3).len(), 8);
        let curto = Quadro {
            dados: &dados[..dados.len() - 1],
            ..q
        };
        assert!(!quadro_valido(&curto));
        let stride_ruim = Quadro {
            stride: 4,
            dados: &dados,
            ..curto
        };
        assert!(!quadro_valido(&stride_ruim));
    }

    #[test]
    fn enum_format_serializa() {
        let bytes = formatos_aceitos();
        assert!(Pod::from_bytes(&bytes).is_some());
    }
}
