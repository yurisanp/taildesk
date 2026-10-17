//! Captura de cada monitor pela Desktop Duplication do DXGI (Windows).
//!
//! Um monitor = uma saída (`IDXGIOutput`) presa à área de trabalho. O `display_id` segue a
//! ordem do monitor principal primeiro e depois a ordem do DXGI (adaptador, saída); o retângulo
//! de cada um ([`MonitorDxgi::area`]) está em pixels físicos da área de trabalho virtual, o mesmo
//! espaço que o mouse por `SendInput` usa ([`crate::mouse::sendinput`]). Para os pixels serem
//! físicos o processo precisa ser DPI-aware por monitor: o manifesto do app declara
//! `PerMonitorV2` e [`listar_monitores`] também pede isso em tempo de execução.
//!
//! Cada monitor capturado tem uma thread com o próprio dispositivo D3D11 no adaptador da
//! saída. O quadro vai da textura da GPU para uma textura de staging reaproveitada e o callback
//! recebe a imagem emprestada do mapeamento, sem alocar. O ponteiro não entra na imagem (a
//! Desktop Duplication nunca o desenha): posição e forma vêm do `DXGI_OUTDUPL_FRAME_INFO` e de
//! `GetFramePointerShape`, e a forma só quando muda.
//!
//! Troca de resolução, tela de UAC ou bloqueio e tela cheia exclusiva derrubam a duplicação
//! (`DXGI_ERROR_ACCESS_LOST`); a thread recria e segue. Monitor pausado (`ativar(id, false)`)
//! solta a duplicação inteira: o Windows não copia nada para ele.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION, DXGI_MODE_ROTATION_ROTATE90,
    DXGI_MODE_ROTATION_ROTATE180, DXGI_MODE_ROTATION_ROTATE270, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_MORE_DATA, DXGI_ERROR_NOT_FOUND,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use crate::captura::{Cursor, FormaCursor, FormatoPixel, Imagem, Quadro, Rotacao};
use crate::forma_cursor_dxgi::{CURSOR_MAX, TipoForma, converter};
use crate::mouse::sendinput::Retangulo;

/// Espera máxima por quadro; também é o tempo para a thread notar que deve parar.
const ESPERA_MS: u32 = 100;
/// Pausa entre tentativas de recriar a duplicação (UAC, bloqueio, troca de modo).
const RECRIAR: Duration = Duration::from_millis(250);

/// Um monitor que dá para capturar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorDxgi {
    pub display_id: u32,
    /// Nome do dispositivo do Windows (`\\.\DISPLAY1`).
    pub nome: String,
    /// Retângulo na área de trabalho virtual, em pixels físicos.
    pub area: Retangulo,
    pub rotacao: Rotacao,
    pub principal: bool,
    adaptador: u32,
    saida: u32,
}

/// Pede DPI por monitor (V2) para o processo. Sem isso o Windows escala as coordenadas de
/// monitores com escala diferente de 100%. Falha em silêncio se o processo já escolheu (manifesto
/// ou chamada anterior), que é o caso normal do app.
pub fn dpi_por_monitor() {
    // SAFETY: sem pré-condições; só muda o modo de DPI do processo, se ainda não foi escolhido.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Monitores ligados à área de trabalho agora.
pub fn listar_monitores() -> io::Result<Vec<MonitorDxgi>> {
    dpi_por_monitor();
    // SAFETY: chamadas COM sem pré-condições além de argumentos válidos, que passamos.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(io_err)?;
    let mut brutos = Vec::new();
    for a in 0.. {
        // SAFETY: idem.
        let adaptador = match unsafe { factory.EnumAdapters1(a) } {
            Ok(ad) => ad,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(io_err(e)),
        };
        for s in 0.. {
            // SAFETY: idem.
            let saida = match unsafe { adaptador.EnumOutputs(s) } {
                Ok(o) => o,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(io_err(e)),
            };
            // SAFETY: idem.
            let desc = unsafe { saida.GetDesc() }.map_err(io_err)?;
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let r = desc.DesktopCoordinates;
            brutos.push(MonitorDxgi {
                display_id: 0,
                nome: nome_utf16(&desc.DeviceName),
                area: Retangulo {
                    x: r.left,
                    y: r.top,
                    largura: r.right - r.left,
                    altura: r.bottom - r.top,
                },
                rotacao: rotacao(desc.Rotation),
                principal: false,
                adaptador: a,
                saida: s,
            });
        }
    }
    Ok(numerar(brutos))
}

/// Principal (o que começa em 0,0) primeiro; os outros na ordem do DXGI.
fn numerar(mut ms: Vec<MonitorDxgi>) -> Vec<MonitorDxgi> {
    for m in &mut ms {
        m.principal = m.area.x == 0 && m.area.y == 0;
    }
    ms.sort_by_key(|m| !m.principal);
    for (i, m) in ms.iter_mut().enumerate() {
        m.display_id = i as u32;
    }
    ms
}

fn nome_utf16(n: &[u16]) -> String {
    let fim = n.iter().position(|&c| c == 0).unwrap_or(n.len());
    String::from_utf16_lossy(&n[..fim])
}

fn rotacao(r: DXGI_MODE_ROTATION) -> Rotacao {
    match r {
        DXGI_MODE_ROTATION_ROTATE90 => Rotacao::Graus90,
        DXGI_MODE_ROTATION_ROTATE180 => Rotacao::Graus180,
        DXGI_MODE_ROTATION_ROTATE270 => Rotacao::Graus270,
        _ => Rotacao::Nenhuma,
    }
}

fn io_err(e: windows::core::Error) -> io::Error {
    io::Error::other(format!("DXGI: {e}"))
}

struct Monitor {
    display_id: u32,
    ativo: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Captura em andamento. Soltar para a captura e espera as threads terminarem.
pub struct CapturaDxgi {
    parar: Arc<AtomicBool>,
    monitores: Vec<Monitor>,
}

impl fmt::Debug for CapturaDxgi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturaDxgi").finish_non_exhaustive()
    }
}

impl CapturaDxgi {
    /// Abre a duplicação de cada monitor e chama `ao_quadro` (uma cópia por monitor, cada uma na
    /// thread dele) a cada imagem ou cursor novo. Falha se algum monitor não abrir.
    pub fn iniciar<F>(monitores: &[MonitorDxgi], ao_quadro: F) -> io::Result<Self>
    where
        F: FnMut(&Quadro<'_>) + Send + Clone + 'static,
    {
        let parar = Arc::new(AtomicBool::new(false));
        let mut captura = CapturaDxgi {
            parar: Arc::clone(&parar),
            monitores: Vec::with_capacity(monitores.len()),
        };
        for m in monitores {
            let (pronto_tx, pronto_rx) = mpsc::sync_channel::<io::Result<()>>(1);
            let ativo = Arc::new(AtomicBool::new(true));
            let alvo = m.clone();
            let (p, a, cb) = (Arc::clone(&parar), Arc::clone(&ativo), ao_quadro.clone());
            let thread = std::thread::Builder::new()
                .name(format!("dxgi-{}", m.display_id))
                .spawn(move || laco(alvo, p, a, pronto_tx, cb))?;
            captura.monitores.push(Monitor {
                display_id: m.display_id,
                ativo,
                thread: Some(thread),
            });
            // Erro aqui: o Drop de `captura` para e junta as threads já abertas.
            pronto_rx
                .recv()
                .map_err(|_| io::Error::other("a thread da captura DXGI terminou"))??;
        }
        Ok(captura)
    }

    /// Pausa (`false`) ou retoma um monitor.
    pub fn ativar(&self, display_id: u32, ativo: bool) {
        if let Some(m) = self.monitores.iter().find(|m| m.display_id == display_id) {
            m.ativo.store(ativo, Ordering::Relaxed);
        }
    }
}

impl Drop for CapturaDxgi {
    fn drop(&mut self) {
        self.parar.store(true, Ordering::Relaxed);
        for m in &mut self.monitores {
            if let Some(t) = m.thread.take() {
                let _ = t.join();
            }
        }
    }
}

enum Falha {
    /// A duplicação caiu (modo, UAC, bloqueio): recriar.
    Perdida,
    Outra(windows::core::Error),
}

impl From<windows::core::Error> for Falha {
    fn from(e: windows::core::Error) -> Self {
        if e.code() == DXGI_ERROR_ACCESS_LOST {
            Falha::Perdida
        } else {
            Falha::Outra(e)
        }
    }
}

fn laco<F>(
    alvo: MonitorDxgi,
    parar: Arc<AtomicBool>,
    ativo: Arc<AtomicBool>,
    pronto: mpsc::SyncSender<io::Result<()>>,
    mut ao_quadro: F,
) where
    F: FnMut(&Quadro<'_>),
{
    // A duplicação só existe na área de trabalho da thread: segue a de entrada (tela de
    // entrada, bloqueio, UAC) quando o processo pode abri-la (agente do serviço).
    crate::mesa_windows::seguir_entrada();
    let mut dup = match Duplicacao::abrir(&alvo) {
        Ok(d) => Some(d),
        Err(e) => {
            let _ = pronto.send(Err(io::Error::other(format!(
                "monitor {} ({}): {e}",
                alvo.display_id, alvo.nome
            ))));
            return;
        }
    };
    let _ = pronto.send(Ok(()));
    // Buffers da forma do cursor, alocados uma vez; o bruto cresce se o Windows pedir mais.
    let mut bruto = vec![0u8; (CURSOR_MAX * CURSOR_MAX * 4) as usize];
    let mut bgra = vec![0u8; (CURSOR_MAX * CURSOR_MAX * 4) as usize];
    while !parar.load(Ordering::Relaxed) {
        if !ativo.load(Ordering::Relaxed) {
            dup = None;
            std::thread::sleep(Duration::from_millis(u64::from(ESPERA_MS)));
            continue;
        }
        let Some(d) = dup.as_mut() else {
            crate::mesa_windows::seguir_entrada();
            match Duplicacao::abrir(&alvo) {
                Ok(nova) => dup = Some(nova),
                Err(_) => std::thread::sleep(RECRIAR),
            }
            continue;
        };
        match d.proximo(alvo.display_id, &mut bruto, &mut bgra, &mut ao_quadro) {
            Ok(()) => {}
            Err(Falha::Perdida) => dup = None,
            Err(Falha::Outra(e)) => {
                eprintln!("captura DXGI do monitor {}: {e}", alvo.display_id);
                dup = None;
                std::thread::sleep(RECRIAR);
            }
        }
    }
}

struct Duplicacao {
    dispositivo: ID3D11Device,
    contexto: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    rotacao: Rotacao,
}

impl Duplicacao {
    fn abrir(alvo: &MonitorDxgi) -> windows::core::Result<Self> {
        // SAFETY: chamadas COM com argumentos válidos; os ponteiros de saída apontam para
        // variáveis locais vivas durante a chamada.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adaptador: IDXGIAdapter1 = factory.EnumAdapters1(alvo.adaptador)?;
            let saida = adaptador.EnumOutputs(alvo.saida)?;
            if nome_utf16(&saida.GetDesc()?.DeviceName) != alvo.nome {
                // Os monitores mudaram desde a listagem.
                return Err(DXGI_ERROR_NOT_FOUND.into());
            }
            let mut dispositivo = None;
            let mut contexto = None;
            D3D11CreateDevice(
                &adaptador,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut dispositivo),
                None,
                Some(&mut contexto),
            )?;
            let (Some(dispositivo), Some(contexto)) = (dispositivo, contexto) else {
                return Err(windows::core::Error::from(DXGI_ERROR_NOT_FOUND));
            };
            let dup = saida
                .cast::<IDXGIOutput1>()?
                .DuplicateOutput(&dispositivo)?;
            let desc = dup.GetDesc();
            Ok(Duplicacao {
                dispositivo,
                contexto,
                dup,
                staging: None,
                rotacao: rotacao(desc.Rotation),
            })
        }
    }

    /// Espera o próximo quadro e chama `ao_quadro` com o que mudou.
    fn proximo<F>(
        &mut self,
        display_id: u32,
        bruto: &mut Vec<u8>,
        bgra: &mut [u8],
        ao_quadro: &mut F,
    ) -> Result<(), Falha>
    where
        F: FnMut(&Quadro<'_>),
    {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut recurso: Option<IDXGIResource> = None;
        // SAFETY: `info` e `recurso` são locais vivos durante a chamada.
        match unsafe {
            self.dup
                .AcquireNextFrame(ESPERA_MS, &mut info, &mut recurso)
        } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        let r = self.entregar(display_id, &info, recurso, bruto, bgra, ao_quadro);
        // SAFETY: há um quadro adquirido acima.
        let solto = unsafe { self.dup.ReleaseFrame() };
        r?;
        solto.map_err(Falha::from)
    }

    fn entregar<F>(
        &mut self,
        display_id: u32,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        recurso: Option<IDXGIResource>,
        bruto: &mut Vec<u8>,
        bgra: &mut [u8],
        ao_quadro: &mut F,
    ) -> Result<(), Falha>
    where
        F: FnMut(&Quadro<'_>),
    {
        let mut forma = None;
        if info.PointerShapeBufferSize > 0 {
            forma = self.ler_forma(info.PointerShapeBufferSize, bruto, bgra)?;
        }
        let cursor = (info.LastMouseUpdateTime != 0).then(|| {
            let p = info.PointerPosition;
            Cursor {
                visivel: p.Visible.as_bool(),
                x: p.Position.x,
                y: p.Position.y,
                forma: forma.map(|(largura, altura, hx, hy)| FormaCursor {
                    largura,
                    altura,
                    stride: largura as usize * 4,
                    hotspot_x: hx,
                    hotspot_y: hy,
                    formato: FormatoPixel::Bgra,
                    dados: &bgra[..(largura * altura * 4) as usize],
                }),
            }
        });
        let textura = match recurso {
            Some(r) if info.LastPresentTime != 0 => Some(r.cast::<ID3D11Texture2D>()?),
            _ => None,
        };
        let Some(textura) = textura else {
            if cursor.is_some() {
                ao_quadro(&Quadro {
                    display_id,
                    imagem: None,
                    cursor,
                });
            }
            return Ok(());
        };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` é local e vivo durante a chamada.
        unsafe { textura.GetDesc(&mut desc) };
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            // A duplicação clássica sempre entrega BGRA 8 bits; HDR fica para o DuplicateOutput1.
            return Err(Falha::Outra(windows::core::Error::from(
                DXGI_ERROR_NOT_FOUND,
            )));
        }
        let staging = self.staging(&desc)?;
        let mut mapa = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: as duas texturas são do mesmo dispositivo, mesmo tamanho e formato; `mapa` é
        // local e vivo durante a chamada.
        unsafe {
            self.contexto.CopyResource(&staging, &textura);
            self.contexto
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapa))?;
        }
        let stride = mapa.RowPitch as usize;
        let tamanho = stride * (desc.Height as usize - 1) + desc.Width as usize * 4;
        // SAFETY: o mapeamento de leitura cobre `RowPitch * Height` bytes a partir de `pData` e
        // fica válido até o `Unmap` abaixo; ninguém escreve na textura de staging no meio.
        let dados = unsafe { std::slice::from_raw_parts(mapa.pData as *const u8, tamanho) };
        ao_quadro(&Quadro {
            display_id,
            imagem: Some(Imagem {
                largura: desc.Width,
                altura: desc.Height,
                stride,
                formato: FormatoPixel::Bgrx,
                rotacao: self.rotacao,
                dados,
            }),
            cursor,
        });
        // SAFETY: mapeado acima; `dados` não é usado depois daqui.
        unsafe { self.contexto.Unmap(&staging, 0) };
        Ok(())
    }

    /// Textura de staging do tamanho do quadro, recriada só quando o tamanho muda.
    fn staging(&mut self, origem: &D3D11_TEXTURE2D_DESC) -> windows::core::Result<ID3D11Texture2D> {
        if let Some(s) = &self.staging {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: `d` é local e vivo durante a chamada.
            unsafe { s.GetDesc(&mut d) };
            if d.Width == origem.Width && d.Height == origem.Height {
                return Ok(s.clone());
            }
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: origem.Width,
            Height: origem.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: origem.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut s = None;
        // SAFETY: `desc` e `s` são locais vivos durante a chamada.
        unsafe {
            self.dispositivo
                .CreateTexture2D(&desc, None, Some(&mut s))?
        };
        let s = s.ok_or_else(|| windows::core::Error::from(DXGI_ERROR_NOT_FOUND))?;
        self.staging = Some(s.clone());
        Ok(s)
    }

    /// Lê a forma nova do ponteiro e converte para BGRA em `bgra`. Devolve
    /// `(largura, altura, hotspot_x, hotspot_y)`, ou `None` se a forma não é aceita.
    fn ler_forma(
        &self,
        tamanho: u32,
        bruto: &mut Vec<u8>,
        bgra: &mut [u8],
    ) -> Result<Option<(u32, u32, i32, i32)>, Falha> {
        if bruto.len() < tamanho as usize {
            bruto.resize(tamanho as usize, 0);
        }
        let mut info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        let mut usado = 0u32;
        // SAFETY: `bruto` tem pelo menos `bruto.len()` bytes graváveis; `usado` e `info` são
        // locais vivos durante a chamada.
        let r = unsafe {
            self.dup.GetFramePointerShape(
                bruto.len() as u32,
                bruto.as_mut_ptr().cast(),
                &mut usado,
                &mut info,
            )
        };
        match r {
            Ok(()) => {}
            // Forma maior que o aceito: ignora (o controlador fica com a anterior).
            Err(e) if e.code() == DXGI_ERROR_MORE_DATA => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let Some(tipo) = TipoForma::de(info.Type) else {
            return Ok(None);
        };
        let origem = &bruto[..(usado as usize).min(bruto.len())];
        Ok(converter(
            tipo,
            info.Width,
            info.Height,
            info.Pitch as usize,
            origem,
            bgra,
        )
        .map(|(l, a)| (l, a, info.HotSpot.x, info.HotSpot.y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(nome: &str, x: i32, y: i32) -> MonitorDxgi {
        MonitorDxgi {
            display_id: 99,
            nome: nome.into(),
            area: Retangulo {
                x,
                y,
                largura: 1920,
                altura: 1080,
            },
            rotacao: Rotacao::Nenhuma,
            principal: false,
            adaptador: 0,
            saida: 0,
        }
    }

    #[test]
    fn principal_vem_primeiro() {
        let ms = numerar(vec![m("A", -1920, 0), m("B", 0, 0), m("C", 1920, 0)]);
        let nomes: Vec<_> = ms
            .iter()
            .map(|m| (m.display_id, m.nome.as_str(), m.principal))
            .collect();
        assert_eq!(nomes, [(0, "B", true), (1, "A", false), (2, "C", false)]);
    }

    #[test]
    fn nome_para_no_nulo() {
        let mut n = [0u16; 32];
        for (i, c) in r"\\.\DISPLAY2".encode_utf16().enumerate() {
            n[i] = c;
        }
        assert_eq!(nome_utf16(&n), r"\\.\DISPLAY2");
    }
}
