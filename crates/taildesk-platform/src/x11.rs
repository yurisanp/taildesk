//! Host numa sessão X11 sem portal (Xorg com XFCE, i3, MATE…), caminho secundário da decisão 5
//! (`13-multimonitor.md` §4.3): monitores pelo RandR, captura em [`crate::captura_x11`] e input
//! em [`crate::entrada_x11`].
//!
//! Tudo fala com o servidor X pelo `x11rb` em Rust puro (sem libxcb). Cada parte abre a própria
//! conexão: a captura numa thread dela, a entrada na tarefa de input da sessão.

use std::io;

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// A sessão gráfica é X11 (e não Wayland): `XDG_SESSION_TYPE=x11`, ou sem `XDG_SESSION_TYPE`
/// com `DISPLAY` e sem `WAYLAND_DISPLAY`.
pub fn sessao_x11() -> bool {
    let var = |n: &str| std::env::var_os(n).filter(|v| !v.is_empty());
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("x11") => var("DISPLAY").is_some(),
        Ok("wayland") => false,
        _ => var("DISPLAY").is_some() && var("WAYLAND_DISPLAY").is_none(),
    }
}

/// Um monitor do RandR, em pixels da janela raiz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorX11 {
    /// Ordem do RandR (o principal primeiro), estável na sessão.
    pub display_id: u32,
    pub nome: String,
    pub x: i32,
    pub y: i32,
    pub largura: u32,
    pub altura: u32,
    pub principal: bool,
}

impl MonitorX11 {
    /// Contém o ponto (`x`, `y`) da raiz.
    pub fn contem(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && i64::from(x) < i64::from(self.x) + i64::from(self.largura)
            && i64::from(y) < i64::from(self.y) + i64::from(self.altura)
    }

    /// Posição normalizada (0..=65535) no monitor → pixel da raiz.
    pub fn ponto(&self, x: u16, y: u16) -> (i16, i16) {
        let eixo = |inicio: i32, tamanho: u32, v: u16| {
            let max = i64::from(tamanho.saturating_sub(1));
            let p = i64::from(inicio) + (i64::from(v) * max + 32767) / 65535;
            p.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
        };
        (eixo(self.x, self.largura, x), eixo(self.y, self.altura, y))
    }
}

/// Erro do X (conexão, protocolo, extensão faltando).
pub fn erro<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(format!("X11: {e}"))
}

/// Abre uma conexão no `DISPLAY` da sessão; devolve também a tela padrão.
pub fn conectar() -> io::Result<(RustConnection, usize)> {
    RustConnection::connect(None).map_err(erro)
}

/// Monitores ativos pelo RandR (1.5+); sem RandR, a tela inteira como um monitor só.
pub fn listar_monitores() -> io::Result<Vec<MonitorX11>> {
    let (conn, tela) = conectar()?;
    listar_na(&conn, tela)
}

pub(crate) fn listar_na(conn: &RustConnection, tela: usize) -> io::Result<Vec<MonitorX11>> {
    let screen = &conn.setup().roots[tela];
    let raiz = screen.root;
    let inteira = || MonitorX11 {
        display_id: 0,
        nome: String::new(),
        x: 0,
        y: 0,
        largura: u32::from(screen.width_in_pixels),
        altura: u32::from(screen.height_in_pixels),
        principal: true,
    };
    let Ok(reply) = conn
        .randr_get_monitors(raiz, true)
        .map_err(erro)
        .and_then(|c| c.reply().map_err(erro))
    else {
        return Ok(vec![inteira()]);
    };
    let mut ms: Vec<MonitorX11> = reply
        .monitors
        .iter()
        .filter(|m| m.width > 0 && m.height > 0)
        .map(|m| MonitorX11 {
            display_id: 0,
            nome: conn
                .get_atom_name(m.name)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                .unwrap_or_default(),
            x: i32::from(m.x),
            y: i32::from(m.y),
            largura: u32::from(m.width),
            altura: u32::from(m.height),
            principal: m.primary,
        })
        .collect();
    if ms.is_empty() {
        return Ok(vec![inteira()]);
    }
    numerar(&mut ms);
    Ok(ms)
}

/// Principal primeiro (ou o que começa em 0,0 se nenhum é principal), depois da esquerda para
/// a direita.
fn numerar(ms: &mut [MonitorX11]) {
    if !ms.iter().any(|m| m.principal)
        && let Some(m) = ms
            .iter_mut()
            .min_by_key(|m| (m.x.abs() + m.y.abs(), m.x, m.y))
    {
        m.principal = true;
    }
    ms.sort_by_key(|m| (!m.principal, m.x, m.y));
    for (i, m) in ms.iter_mut().enumerate() {
        m.display_id = i as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(x: i32, principal: bool) -> MonitorX11 {
        MonitorX11 {
            display_id: 99,
            nome: format!("M{x}"),
            x,
            y: 0,
            largura: 1920,
            altura: 1080,
            principal,
        }
    }

    #[test]
    fn principal_primeiro_e_depois_da_esquerda() {
        let mut ms = vec![m(1920, false), m(-1920, false), m(0, true)];
        numerar(&mut ms);
        let ids: Vec<_> = ms.iter().map(|m| (m.display_id, m.x)).collect();
        assert_eq!(ids, [(0, 0), (1, -1920), (2, 1920)]);
        // Sem principal marcado, o mais perto da origem vira o principal.
        let mut ms = vec![m(3840, false), m(1920, false)];
        numerar(&mut ms);
        assert!(ms[0].principal && ms[0].x == 1920);
    }

    #[test]
    fn ponto_normalizado_vai_para_a_raiz() {
        let d = m(1920, false);
        assert_eq!(d.ponto(0, 0), (1920, 0));
        assert_eq!(d.ponto(65535, 65535), (1920 + 1919, 1079));
        assert_eq!(d.ponto(32768, 0).0, 1920 + 960);
        assert!(d.contem(1920, 0) && d.contem(3839, 1079));
        assert!(!d.contem(3840, 0) && !d.contem(1919, 5));
    }
}
