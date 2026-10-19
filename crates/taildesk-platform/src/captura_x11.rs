//! Captura dos monitores numa sessão X11 (`13-multimonitor.md` §4.3), para quando não há portal
//! (XFCE, i3, MATE em Xorg).
//!
//! Uma thread com a própria conexão: a cada ~16 ms lê os eventos do XDamage na raiz e copia, de
//! cada monitor ativo que mudou, o sub-retângulo da raiz para um segmento MIT-SHM (memória
//! compartilhada com o servidor, sem passar pelo socket). O callback recebe a imagem emprestada
//! do segmento, sem alocar. Sem MIT-SHM (X remoto) cai para `GetImage` pelo socket; sem XDamage,
//! copia a 30 quadros por segundo mesmo parado.
//!
//! O cursor vem do XFixes (`GetCursorImage`): posição em todo passo e o bitmap só quando o
//! número de série muda. A imagem sai sem o cursor, como no portal e no DXGI.

use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _, ImageFormat, ImageOrder};
use x11rb::rust_connection::RustConnection;

pub use crate::captura::{Cursor, FormaCursor, FormatoPixel, Imagem, Quadro, Rotacao};
use crate::x11::{MonitorX11, conectar, erro};

/// Passo do laço: lê dano e cursor a ~60 Hz.
const PASSO: Duration = Duration::from_millis(16);
/// Sem XDamage, copia a esta taxa mesmo sem mudança.
const PASSO_SEM_DANO: Duration = Duration::from_millis(33);

enum Comando {
    Ativar(u32, bool),
    Parar,
}

/// Captura em andamento. Soltar para a thread.
pub struct CapturaX11 {
    tx: mpsc::Sender<Comando>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for CapturaX11 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturaX11").finish_non_exhaustive()
    }
}

impl CapturaX11 {
    /// Começa a capturar `monitores` (todos ativos) e chama `ao_quadro` na thread da captura.
    pub fn iniciar<F>(monitores: &[MonitorX11], ao_quadro: F) -> io::Result<Self>
    where
        F: FnMut(&Quadro<'_>) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let (pronto_tx, pronto_rx) = mpsc::sync_channel::<io::Result<()>>(1);
        let monitores = monitores.to_vec();
        let thread = std::thread::Builder::new()
            .name("taildesk-x11".into())
            .spawn(move || match Laco::abrir(monitores) {
                Ok(mut l) => {
                    let _ = pronto_tx.send(Ok(()));
                    l.rodar(&rx, ao_quadro);
                }
                Err(e) => {
                    let _ = pronto_tx.send(Err(e));
                }
            })?;
        match pronto_rx.recv() {
            Ok(Ok(())) => Ok(CapturaX11 {
                tx,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(io::Error::other(
                    "a thread da captura X11 terminou antes de começar",
                ))
            }
        }
    }

    /// Pausa ou retoma um monitor. Pausado, nada é copiado dele.
    pub fn ativar(&self, display_id: u32, ativo: bool) {
        let _ = self.tx.send(Comando::Ativar(display_id, ativo));
    }
}

impl Drop for CapturaX11 {
    fn drop(&mut self) {
        let _ = self.tx.send(Comando::Parar);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Segmento MIT-SHM mapeado aqui (só leitura).
struct Segmento {
    id: u32,
    ptr: *mut std::ffi::c_void,
    tamanho: usize,
    _fd: OwnedFd,
}

impl Segmento {
    fn abrir(conn: &RustConnection, tamanho: usize) -> io::Result<Segmento> {
        conn.shm_query_version()
            .map_err(erro)?
            .reply()
            .map_err(erro)?;
        let id = conn.generate_id().map_err(erro)?;
        let tam32 =
            u32::try_from(tamanho).map_err(|_| io::Error::other("segmento grande demais"))?;
        let reply = conn
            .shm_create_segment(id, tam32, false)
            .map_err(erro)?
            .reply()
            .map_err(erro)?;
        let fd: OwnedFd = reply.shm_fd;
        // SAFETY: mapeia `tamanho` bytes do segmento que o servidor acabou de criar com esse
        // tamanho; a região fica válida até o `munmap` no `Drop`, e só lemos dela.
        let ptr = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                tamanho,
                rustix::mm::ProtFlags::READ,
                rustix::mm::MapFlags::SHARED,
                fd.as_fd(),
                0,
            )
        }
        .map_err(io::Error::from)?;
        Ok(Segmento {
            id,
            ptr,
            tamanho,
            _fd: fd,
        })
    }

    /// Os primeiros `n` bytes, como o servidor deixou no último `ShmGetImage`.
    fn dados(&self, n: usize) -> &[u8] {
        // SAFETY: `ptr` aponta para `tamanho` bytes mapeados enquanto `self` vive; `n` é
        // limitado a isso. O servidor só escreve durante um `ShmGetImage`, que já respondeu.
        unsafe { std::slice::from_raw_parts(self.ptr.cast::<u8>(), n.min(self.tamanho)) }
    }
}

impl Drop for Segmento {
    fn drop(&mut self) {
        // SAFETY: desfaz o `mmap` de `abrir`, com o mesmo ponteiro e tamanho; nada mais usa a
        // região depois daqui.
        let _ = unsafe { rustix::mm::munmap(self.ptr, self.tamanho) };
    }
}

// SAFETY: o ponteiro é de um mapeamento próprio do segmento, usado só pela thread da captura.
unsafe impl Send for Segmento {}

/// Cursor do XFixes convertido para BGRA com alfa direto (o XFixes manda pré-multiplicado).
#[derive(Default)]
struct CursorX {
    serie: Option<u32>,
    pos: (i32, i32),
    hot: (i32, i32),
    tamanho: (u32, u32),
    bgra: Vec<u8>,
    /// Monitor onde o cursor estava por último (para avisar quando sai dele).
    em: Option<u32>,
}

/// ARGB pré-multiplicado (um `u32` por pixel) → BGRA com alfa direto.
pub(crate) fn cursor_bgra(argb: &[u32], saida: &mut Vec<u8>) {
    saida.clear();
    for &p in argb {
        let a = (p >> 24) as u8;
        let canal = |c: u32| -> u8 {
            let c = (c & 0xFF) as u16;
            if a == 0 {
                0
            } else {
                ((c * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8
            }
        };
        saida.extend_from_slice(&[canal(p), canal(p >> 8), canal(p >> 16), a]);
    }
}

struct Laco {
    conn: RustConnection,
    raiz: xproto::Window,
    monitores: Vec<MonitorX11>,
    ativos: Vec<bool>,
    sujos: Vec<bool>,
    shm: Option<Segmento>,
    dano: Option<damage::Damage>,
    xfixes: bool,
    cursor: CursorX,
}

impl Laco {
    fn abrir(monitores: Vec<MonitorX11>) -> io::Result<Laco> {
        let (conn, tela) = conectar()?;
        let screen = &conn.setup().roots[tela];
        let raiz = screen.root;
        let profundidade = screen.root_depth;
        let bpp = conn
            .setup()
            .pixmap_formats
            .iter()
            .find(|f| f.depth == profundidade)
            .map(|f| f.bits_per_pixel);
        if bpp != Some(32) || conn.setup().image_byte_order != ImageOrder::LSB_FIRST {
            return Err(io::Error::other(format!(
                "X11: só telas de 32 bits por pixel em little-endian (profundidade {profundidade})"
            )));
        }
        let maior = monitores
            .iter()
            .map(|m| m.largura as usize * m.altura as usize * 4)
            .max()
            .unwrap_or(0);
        let shm = Segmento::abrir(&conn, maior.max(4))
            .inspect_err(|e| eprintln!("taildesk: MIT-SHM indisponível ({e}); captura pelo socket"))
            .ok();
        let dano = (|| -> io::Result<damage::Damage> {
            conn.damage_query_version(1, 1)
                .map_err(erro)?
                .reply()
                .map_err(erro)?;
            let d = conn.generate_id().map_err(erro)?;
            conn.damage_create(d, raiz, damage::ReportLevel::BOUNDING_BOX)
                .map_err(erro)?
                .check()
                .map_err(erro)?;
            Ok(d)
        })()
        .inspect_err(|e| eprintln!("taildesk: XDamage indisponível ({e}); captura contínua"))
        .ok();
        let xfixes = conn
            .xfixes_query_version(4, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some();
        let n = monitores.len();
        Ok(Laco {
            conn,
            raiz,
            monitores,
            ativos: vec![true; n],
            sujos: vec![true; n],
            shm,
            dano,
            xfixes,
            cursor: CursorX::default(),
        })
    }

    fn rodar<F>(&mut self, rx: &mpsc::Receiver<Comando>, mut ao_quadro: F)
    where
        F: FnMut(&Quadro<'_>),
    {
        let passo = if self.dano.is_some() {
            PASSO
        } else {
            PASSO_SEM_DANO
        };
        loop {
            let inicio = Instant::now();
            loop {
                match rx.try_recv() {
                    Ok(Comando::Ativar(id, ativo)) => {
                        if let Some(i) = self.monitores.iter().position(|m| m.display_id == id) {
                            self.ativos[i] = ativo;
                            // Retomado: um quadro inteiro logo, mesmo parado.
                            self.sujos[i] |= ativo;
                        }
                    }
                    Ok(Comando::Parar) | Err(mpsc::TryRecvError::Disconnected) => return,
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            self.ler_dano();
            for i in 0..self.monitores.len() {
                if self.ativos[i] && std::mem::take(&mut self.sujos[i]) {
                    self.copiar(i, &mut ao_quadro);
                }
            }
            if self.xfixes {
                self.ler_cursor(&mut ao_quadro);
            }
            if let Some(resto) = passo.checked_sub(inicio.elapsed()) {
                std::thread::sleep(resto);
            }
        }
    }

    /// Marca os monitores que o retângulo do dano toca; sem XDamage, todos.
    fn ler_dano(&mut self) {
        let Some(d) = self.dano else {
            self.sujos.iter_mut().for_each(|s| *s = true);
            return;
        };
        let mut houve = false;
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            if let Event::DamageNotify(n) = ev {
                houve = true;
                let a = n.area;
                let (x0, y0) = (i32::from(a.x), i32::from(a.y));
                let (x1, y1) = (x0 + i32::from(a.width), y0 + i32::from(a.height));
                for (m, sujo) in self.monitores.iter().zip(&mut self.sujos) {
                    let (mx1, my1) = (m.x + m.largura as i32, m.y + m.altura as i32);
                    if x0 < mx1 && x1 > m.x && y0 < my1 && y1 > m.y {
                        *sujo = true;
                    }
                }
            }
        }
        if houve {
            // Zera o dano acumulado: o próximo evento vem com a próxima mudança.
            let _ = self.conn.damage_subtract(d, x11rb::NONE, x11rb::NONE);
            let _ = self.conn.flush();
        }
    }

    fn copiar<F>(&mut self, i: usize, ao_quadro: &mut F)
    where
        F: FnMut(&Quadro<'_>),
    {
        let m = &self.monitores[i];
        let (x, y) = (m.x as i16, m.y as i16);
        let (w, h) = (m.largura as u16, m.altura as u16);
        let n = usize::from(w) * usize::from(h) * 4;
        let id = m.display_id;
        fn quadro(display_id: u32, w: u16, h: u16, dados: &[u8]) -> Quadro<'_> {
            Quadro {
                display_id,
                imagem: Some(Imagem {
                    largura: u32::from(w),
                    altura: u32::from(h),
                    stride: usize::from(w) * 4,
                    formato: FormatoPixel::Bgrx,
                    rotacao: Rotacao::Nenhuma,
                    dados,
                }),
                cursor: None,
            }
        }
        if let Some(s) = &self.shm {
            let ok = self
                .conn
                .shm_get_image(
                    self.raiz,
                    x,
                    y,
                    w,
                    h,
                    !0,
                    ImageFormat::Z_PIXMAP.into(),
                    s.id,
                    0,
                )
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some();
            if ok {
                ao_quadro(&quadro(id, w, h, s.dados(n)));
            }
            return;
        }
        if let Ok(r) = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.raiz, x, y, w, h, !0)
            .map_err(erro)
            .and_then(|c| c.reply().map_err(erro))
            && r.data.len() >= n
        {
            ao_quadro(&quadro(id, w, h, &r.data[..n]));
        }
    }

    fn ler_cursor<F>(&mut self, ao_quadro: &mut F)
    where
        F: FnMut(&Quadro<'_>),
    {
        let Some(r) = self
            .conn
            .xfixes_get_cursor_image()
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            return;
        };
        let c = &mut self.cursor;
        let pos = (i32::from(r.x), i32::from(r.y));
        let forma_nova = c.serie != Some(r.cursor_serial);
        if forma_nova {
            c.serie = Some(r.cursor_serial);
            c.hot = (i32::from(r.xhot), i32::from(r.yhot));
            c.tamanho = (u32::from(r.width), u32::from(r.height));
            cursor_bgra(&r.cursor_image, &mut c.bgra);
        }
        let agora = self
            .monitores
            .iter()
            .zip(&self.ativos)
            .find(|(m, ativo)| **ativo && m.contem(pos.0, pos.1))
            .map(|(m, _)| m.display_id);
        if !forma_nova && pos == c.pos && agora == c.em {
            return;
        }
        // Saiu do monitor onde estava: lá ele some.
        if let Some(antes) = c.em.filter(|a| Some(*a) != agora) {
            ao_quadro(&Quadro {
                display_id: antes,
                imagem: None,
                cursor: Some(Cursor {
                    visivel: false,
                    x: 0,
                    y: 0,
                    forma: None,
                }),
            });
        }
        let mudou_de_monitor = agora != c.em;
        c.pos = pos;
        c.em = agora;
        let Some(id) = agora else { return };
        let Some(m) = self.monitores.iter().find(|m| m.display_id == id) else {
            return;
        };
        let forma = (forma_nova || mudou_de_monitor).then(|| FormaCursor {
            largura: c.tamanho.0,
            altura: c.tamanho.1,
            stride: c.tamanho.0 as usize * 4,
            hotspot_x: c.hot.0,
            hotspot_y: c.hot.1,
            formato: FormatoPixel::Bgra,
            dados: &c.bgra,
        });
        ao_quadro(&Quadro {
            display_id: id,
            imagem: None,
            cursor: Some(Cursor {
                visivel: true,
                x: pos.0 - m.x,
                y: pos.1 - m.y,
                forma,
            }),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn cursor_premultiplicado_vira_alfa_direto() {
        let mut out = Vec::new();
        // Opaco vermelho, transparente, meio branco (128 pré-multiplicado = 255 direto).
        cursor_bgra(&[0xFFFF_0000, 0x0000_0000, 0x8080_8080], &mut out);
        assert_eq!(out, [0, 0, 255, 255, 0, 0, 0, 0, 255, 255, 255, 128]);
    }

    /// Com um servidor X (`DISPLAY`, ex.: Xvfb), captura um quadro de cada monitor. Sem X, o
    /// teste passa sem testar.
    #[test]
    fn captura_um_quadro_no_servidor_x() {
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let Ok(ms) = crate::x11::listar_monitores() else {
            return;
        };
        let vistos = Arc::new(Mutex::new(Vec::new()));
        let v = vistos.clone();
        let c = CapturaX11::iniciar(&ms, move |q| {
            if let Some(i) = &q.imagem {
                assert_eq!(i.dados.len(), i.largura as usize * i.altura as usize * 4);
                v.lock().unwrap().push((q.display_id, i.largura, i.altura));
            }
        })
        .expect("captura X11");
        let fim = Instant::now() + Duration::from_secs(5);
        while vistos.lock().unwrap().len() < ms.len() && Instant::now() < fim {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(c);
        let vistos = vistos.lock().unwrap();
        for m in &ms {
            assert!(
                vistos.contains(&(m.display_id, m.largura, m.altura)),
                "{m:?} não chegou: {vistos:?}"
            );
        }
    }
}
