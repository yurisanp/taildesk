//! Área de transferência do Windows (Win32), numa thread com uma janela só de mensagens.
//!
//! - Mudanças: `AddClipboardFormatListener` → `WM_CLIPBOARDUPDATE`; as que nós mesmos fizemos
//!   (dono = nossa janela) não são avisadas, nem as marcadas pelos gerenciadores de senha
//!   (`ExcludeClipboardContentFromMonitorProcessing`).
//! - Oferta do peer: renderização atrasada (`SetClipboardData(formato, NULL)`); quando um app
//!   cola chega `WM_RENDERFORMAT` e o conteúdo é pedido ao peer ali mesmo, com prazo.
//! - Formatos: texto (`CF_UNICODETEXT`), HTML (`"HTML Format"`), imagem (`"PNG"`, e `CF_DIB`
//!   para os apps que só leem bitmap) e arquivos (`CF_HDROP`, com `"Preferred DropEffect"` =
//!   copiar). As conversões ficam em [`crate::area_formatos`].

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use taildesk_proto::control::ClipFormat;
use taildesk_proto::control::limits::CLIP_TOTAL;
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    RemoveClipboardFormatListener, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW,
    GetWindowLongPtrW, HWND_MESSAGE, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    SetWindowLongPtrW, WM_APP, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY, WM_RENDERFORMAT,
    WNDCLASSEXW,
};

use crate::area_formatos as fmt;

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;
/// `DROPEFFECT_COPY`.
const COPIAR: u32 = 1;
/// Há comandos na fila.
const WM_COMANDO: u32 = WM_APP + 1;

/// O que a área de transferência avisa (na thread dela: só repasse).
#[derive(Debug)]
pub enum DaArea {
    /// Alguém copiou em outro app; os formatos que dá para levar e, com
    /// [`ClipFormat::Files`], os caminhos copiados.
    Copiou(Vec<ClipFormat>, Vec<PathBuf>),
}

/// Pede ao peer o conteúdo de um formato da oferta em vigor (bloqueia a thread da área até
/// voltar; `None` = colar vazio). Texto e HTML em UTF-8, imagem em PNG, arquivos como os
/// caminhos locais já baixados, um por linha.
pub type Pedir = Box<dyn FnMut(ClipFormat) -> Option<Vec<u8>> + Send>;

enum Comando {
    Oferecer(Vec<ClipFormat>),
    Ler(ClipFormat, mpsc::Sender<io::Result<Vec<u8>>>),
}

/// A área de transferência; soltar fecha a janela e para a thread.
#[derive(Debug)]
pub struct AreaWindows {
    hwnd: isize,
    comandos: mpsc::Sender<Comando>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Comando {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Comando::Oferecer(v) => f.debug_tuple("Oferecer").field(v).finish(),
            Comando::Ler(c, _) => f.debug_tuple("Ler").field(c).finish(),
        }
    }
}

impl Drop for AreaWindows {
    fn drop(&mut self) {
        // SAFETY: a janela é da nossa thread; mandar WM_CLOSE para uma janela já destruída só
        // falha.
        unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ids dos formatos registrados (iguais em todo o sistema enquanto ele está ligado).
#[derive(Clone, Copy, Debug)]
struct Registrados {
    html: u32,
    png: u32,
    excluir: u32,
    efeito: u32,
}

impl Registrados {
    fn novos() -> Registrados {
        // SAFETY: strings terminadas em zero que vivem durante a chamada.
        unsafe {
            Registrados {
                html: RegisterClipboardFormatW(wide("HTML Format").as_ptr()),
                png: RegisterClipboardFormatW(wide("PNG").as_ptr()),
                excluir: RegisterClipboardFormatW(
                    wide("ExcludeClipboardContentFromMonitorProcessing").as_ptr(),
                ),
                efeito: RegisterClipboardFormatW(wide("Preferred DropEffect").as_ptr()),
            }
        }
    }

    fn do_windows(&self, f: ClipFormat) -> Vec<u32> {
        match f {
            ClipFormat::Text => vec![CF_UNICODETEXT],
            ClipFormat::Html => vec![self.html],
            ClipFormat::Png => vec![self.png, CF_DIB],
            ClipFormat::Files => vec![CF_HDROP],
            _ => Vec::new(),
        }
    }

    fn do_protocolo(&self, id: u32) -> Option<ClipFormat> {
        match id {
            CF_UNICODETEXT => Some(ClipFormat::Text),
            CF_DIB | CF_DIBV5 => Some(ClipFormat::Png),
            CF_HDROP => Some(ClipFormat::Files),
            _ if id == self.html => Some(ClipFormat::Html),
            _ if id == self.png => Some(ClipFormat::Png),
            _ => None,
        }
    }
}

struct Estado {
    hwnd: HWND,
    ids: Registrados,
    comandos: mpsc::Receiver<Comando>,
    aviso: Box<dyn FnMut(DaArea) + Send>,
    pedir: Pedir,
    /// Conteúdo já pedido ao peer na oferta em vigor (PNG serve ao "PNG" e ao `CF_DIB`).
    cache: HashMap<ClipFormat, Option<Vec<u8>>>,
}

impl AreaWindows {
    /// Abre a área de transferência. `aviso` e `pedir` rodam na thread dela.
    pub fn abrir(
        aviso: impl FnMut(DaArea) + Send + 'static,
        pedir: impl FnMut(ClipFormat) -> Option<Vec<u8>> + Send + 'static,
    ) -> io::Result<AreaWindows> {
        let (comandos, rx) = mpsc::channel();
        let (pronto_tx, pronto_rx) = mpsc::sync_channel(1);
        let aviso: Box<dyn FnMut(DaArea) + Send> = Box::new(aviso);
        let pedir: Pedir = Box::new(pedir);
        let thread = std::thread::Builder::new()
            .name("taildesk-area".into())
            .spawn(move || {
                // SAFETY: a thread é dona da janela e do estado até o fim do laço.
                unsafe { rodar(rx, aviso, pedir, &pronto_tx) }
            })?;
        let hwnd = pronto_rx
            .recv()
            .map_err(|_| io::Error::other("área de transferência terminou"))??;
        Ok(AreaWindows {
            hwnd,
            comandos,
            thread: Some(thread),
        })
    }

    fn mandar(&self, c: Comando) -> io::Result<()> {
        self.comandos
            .send(c)
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        // SAFETY: só posta uma mensagem; janela já destruída faz falhar.
        unsafe { PostMessageW(self.hwnd as HWND, WM_COMANDO, 0, 0) };
        Ok(())
    }

    /// Põe a oferta do peer (sem o conteúdo, que vem por `pedir` quando alguém colar).
    pub fn oferecer(&self, formatos: Vec<ClipFormat>) -> io::Result<()> {
        self.mandar(Comando::Oferecer(formatos))
    }

    /// Lê um formato do que está copiado aqui (bloqueia).
    pub fn ler(&self, f: ClipFormat) -> io::Result<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        self.mandar(Comando::Ler(f, tx))?;
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
    }
}

unsafe fn rodar(
    comandos: mpsc::Receiver<Comando>,
    aviso: Box<dyn FnMut(DaArea) + Send>,
    pedir: Pedir,
    pronto: &mpsc::SyncSender<io::Result<isize>>,
) {
    let classe = wide("TailDeskAreaTransferencia");
    // SAFETY: estruturas preenchidas aqui, strings vivas durante as chamadas.
    let hwnd = unsafe {
        let instancia = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_janela),
            hInstance: instancia,
            lpszClassName: classe.as_ptr(),
            // SAFETY: zero é válido para os outros campos (sem ícone, cursor, menu).
            ..std::mem::zeroed()
        };
        // Registrar de novo (segunda área no mesmo processo) falha e a classe antiga vale.
        RegisterClassExW(&wc);
        CreateWindowExW(
            0,
            classe.as_ptr(),
            classe.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instancia,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        let _ = pronto.send(Err(io::Error::last_os_error()));
        return;
    }
    let estado = Box::new(Estado {
        hwnd,
        ids: Registrados::novos(),
        comandos,
        aviso,
        pedir,
        cache: HashMap::new(),
    });
    // SAFETY: o ponteiro vive até o WM_DESTROY, que o devolve ao Box; a janela é desta thread.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(estado) as isize);
        if AddClipboardFormatListener(hwnd) == 0 {
            let erro = io::Error::last_os_error();
            DestroyWindow(hwnd);
            let _ = pronto.send(Err(erro));
            return;
        }
    }
    let _ = pronto.send(Ok(hwnd as isize));
    // SAFETY: laço de mensagens padrão da thread dona da janela.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn proc_janela(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: o ponteiro foi posto por `rodar` e só é solto no WM_DESTROY, nesta mesma thread.
    let estado = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Estado).as_mut() };
    let Some(e) = estado else {
        // SAFETY: repasse padrão.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_CLIPBOARDUPDATE => {
            e.mudou();
            0
        }
        WM_RENDERFORMAT => {
            e.renderizar(wparam as u32);
            0
        }
        WM_COMANDO => {
            while let Ok(c) = e.comandos.try_recv() {
                e.comando(c);
            }
            0
        }
        WM_CLOSE => {
            // SAFETY: a janela é desta thread.
            unsafe {
                RemoveClipboardFormatListener(hwnd);
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            // SAFETY: devolve o Box posto em `rodar`; ninguém mais usa o ponteiro.
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(e as *mut Estado));
                PostQuitMessage(0);
            }
            0
        }
        // SAFETY: repasse padrão.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Abre a área de transferência (outro app pode estar com ela: tenta por um tempo).
fn abrir_area(hwnd: HWND) -> bool {
    for _ in 0..20 {
        // SAFETY: a janela é desta thread.
        if unsafe { OpenClipboard(hwnd) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Fecha ao sair do escopo.
struct Aberta;

impl Drop for Aberta {
    fn drop(&mut self) {
        // SAFETY: só existe depois de um `OpenClipboard` que deu certo nesta thread.
        unsafe { CloseClipboard() };
    }
}

fn disponivel(id: u32) -> bool {
    // SAFETY: sem pré-condições.
    unsafe { IsClipboardFormatAvailable(id) != 0 }
}

/// Bytes de um formato da área de transferência aberta.
fn ler_global(id: u32) -> Option<Vec<u8>> {
    // SAFETY: área aberta por quem chama; o HGLOBAL é do sistema e só é lido entre o
    // `GlobalLock` e o `GlobalUnlock`.
    unsafe {
        let h = GetClipboardData(id);
        if h.is_null() {
            return None;
        }
        let tam = GlobalSize(h);
        if tam == 0 || tam as u64 > CLIP_TOTAL {
            return None;
        }
        let p = GlobalLock(h) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, tam).to_vec();
        GlobalUnlock(h);
        Some(v)
    }
}

/// Um HGLOBAL móvel com `dados`, para o `SetClipboardData`.
fn global_de(dados: &[u8]) -> Option<HANDLE> {
    // SAFETY: memória alocada aqui com o tamanho certo; a cópia fica dentro dela.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, dados.len().max(1));
        if h.is_null() {
            return None;
        }
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            GlobalFree(h);
            return None;
        }
        std::ptr::copy_nonoverlapping(dados.as_ptr(), p, dados.len());
        GlobalUnlock(h);
        Some(h)
    }
}

impl Estado {
    fn mudou(&mut self) {
        // SAFETY: sem pré-condições.
        if unsafe { GetClipboardOwner() } == self.hwnd {
            return;
        }
        if disponivel(self.ids.excluir) {
            return;
        }
        let mut formatos = Vec::new();
        if disponivel(CF_UNICODETEXT) {
            formatos.push(ClipFormat::Text);
        }
        if disponivel(self.ids.html) {
            formatos.push(ClipFormat::Html);
        }
        if disponivel(self.ids.png) || disponivel(CF_DIBV5) || disponivel(CF_DIB) {
            formatos.push(ClipFormat::Png);
        }
        let mut caminhos = Vec::new();
        if disponivel(CF_HDROP) {
            caminhos = self.caminhos();
            if !caminhos.is_empty() {
                formatos.push(ClipFormat::Files);
            }
        }
        if !formatos.is_empty() {
            (self.aviso)(DaArea::Copiou(formatos, caminhos));
        }
    }

    /// Os arquivos copiados (`CF_HDROP`).
    fn caminhos(&self) -> Vec<PathBuf> {
        if !abrir_area(self.hwnd) {
            return Vec::new();
        }
        let _aberta = Aberta;
        ler_global(CF_HDROP)
            .map(|b| {
                fmt::hdrop_para_caminhos(&b)
                    .into_iter()
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn comando(&mut self, c: Comando) {
        match c {
            Comando::Oferecer(formatos) => {
                self.cache.clear();
                if !abrir_area(self.hwnd) {
                    return;
                }
                let _aberta = Aberta;
                // SAFETY: área aberta por nós; NULL = renderização atrasada.
                unsafe {
                    EmptyClipboard();
                    for &f in &formatos {
                        for id in self.ids.do_windows(f) {
                            SetClipboardData(id, std::ptr::null_mut());
                        }
                    }
                    // Colar arquivos copia (não move); vai já, é pequeno.
                    if formatos.contains(&ClipFormat::Files)
                        && let Some(h) = global_de(&COPIAR.to_le_bytes())
                        && SetClipboardData(self.ids.efeito, h).is_null()
                    {
                        GlobalFree(h);
                    }
                }
            }
            Comando::Ler(f, resposta) => {
                let _ = resposta.send(self.ler(f));
            }
        }
    }

    fn ler(&self, f: ClipFormat) -> io::Result<Vec<u8>> {
        let nao = || io::Error::from(io::ErrorKind::NotFound);
        if !abrir_area(self.hwnd) {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        let _aberta = Aberta;
        match f {
            ClipFormat::Text => {
                let b = ler_global(CF_UNICODETEXT).ok_or_else(nao)?;
                let u: Vec<u16> = b
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                Ok(fmt::utf16_para_texto(&u))
            }
            ClipFormat::Html => {
                let b = ler_global(self.ids.html).ok_or_else(nao)?;
                fmt::cf_html_para_fragmento(&b).ok_or_else(nao)
            }
            ClipFormat::Png => {
                if let Some(png) = ler_global(self.ids.png) {
                    return Ok(png);
                }
                let dib = ler_global(CF_DIBV5)
                    .or_else(|| ler_global(CF_DIB))
                    .ok_or_else(nao)?;
                fmt::dib_para_rgba(&dib)
                    .and_then(|img| fmt::rgba_para_png(&img))
                    .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
            }
            ClipFormat::Files => {
                let b = ler_global(CF_HDROP).ok_or_else(nao)?;
                Ok(fmt::hdrop_para_caminhos(&b).join("\n").into_bytes())
            }
            _ => Err(nao()),
        }
    }

    /// Um app colou um formato da nossa oferta: pede ao peer e entrega (área já aberta pelo
    /// app que colou).
    fn renderizar(&mut self, id: u32) {
        let Some(f) = self.ids.do_protocolo(id) else {
            return;
        };
        let dados = match self.cache.get(&f) {
            Some(d) => d.clone(),
            None => {
                let d = (self.pedir)(f);
                self.cache.insert(f, d.clone());
                d
            }
        };
        let Some(dados) = dados else { return };
        let convertido = match id {
            CF_UNICODETEXT => Some(
                fmt::texto_para_utf16(&dados)
                    .iter()
                    .flat_map(|c| c.to_le_bytes())
                    .collect(),
            ),
            CF_DIB => fmt::png_para_rgba(&dados).map(|img| fmt::rgba_para_dib(&img)),
            CF_HDROP => {
                let caminhos: Vec<String> = String::from_utf8_lossy(&dados)
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
                Some(fmt::caminhos_para_hdrop(&caminhos))
            }
            _ if id == self.ids.html => {
                let mut v = fmt::fragmento_para_cf_html(&dados);
                v.push(0);
                Some(v)
            }
            _ => Some(dados),
        };
        let Some(h) = convertido.as_deref().and_then(global_de) else {
            return;
        };
        // SAFETY: no WM_RENDERFORMAT a área já está aberta para nós; com sucesso o sistema
        // fica dono do HGLOBAL, sem sucesso ele volta para nós.
        unsafe {
            if SetClipboardData(id, h).is_null() {
                GlobalFree(h);
            }
        }
    }
}
