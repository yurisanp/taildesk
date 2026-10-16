//! Área de transferência: o que os backends de cada sistema têm em comum.
//!
//! - Linux host: portal `Clipboard` da sessão RemoteDesktop ([`crate::wayland::SessaoPortal`]).
//! - Linux controlador (Wayland): `wl_data_device` da janela da sessão ([`crate::area_wayland`]).
//! - Windows: a área de transferência do Win32 ([`crate::area_windows`]).
//!
//! No Linux tudo é por tipo MIME; aqui fica a tradução entre os MIME e os formatos do
//! protocolo, e o que normalizar no texto.

use taildesk_proto::control::ClipFormat;

/// Tipos MIME de cada formato, do preferido para o último. O primeiro é o que se lê quando o
/// app que copiou oferece vários; todos são anunciados quando a oferta é nossa.
pub fn mimes(f: ClipFormat) -> &'static [&'static str] {
    match f {
        ClipFormat::Text => &[
            "text/plain;charset=utf-8",
            "UTF8_STRING",
            "text/plain",
            "STRING",
            "TEXT",
        ],
        ClipFormat::Html => &["text/html"],
        ClipFormat::Png => &["image/png"],
        // O segundo é o que o Nautilus e afins leem ao colar ("copy" + URIs).
        ClipFormat::Files => &["text/uri-list", "x-special/gnome-copied-files"],
        ClipFormat::Unspecified => &[],
    }
}

/// Formato do protocolo de um tipo MIME (parâmetros como `;charset=` são aceitos no texto).
pub fn formato_do_mime(mime: &str) -> Option<ClipFormat> {
    let base = mime.split(';').next().unwrap_or("").trim();
    [
        ClipFormat::Text,
        ClipFormat::Html,
        ClipFormat::Png,
        ClipFormat::Files,
    ]
    .into_iter()
    .find(|&f| {
        mimes(f)
            .iter()
            .any(|m| *m == mime || m.split(';').next() == Some(base))
    })
}

/// Formatos presentes numa oferta do sistema, na ordem do protocolo e sem repetir.
pub fn formatos(oferta: &[String]) -> Vec<ClipFormat> {
    let mut v: Vec<ClipFormat> = oferta.iter().filter_map(|m| formato_do_mime(m)).collect();
    v.sort();
    v.dedup();
    v
}

/// O MIME a ler para `f` dentre os que a oferta tem (o preferido de [`mimes`]).
pub fn mime_para_ler(f: ClipFormat, oferta: &[String]) -> Option<&str> {
    mimes(f)
        .iter()
        .find(|m| oferta.iter().any(|o| o == *m))
        .copied()
        .or_else(|| {
            oferta
                .iter()
                .find(|o| formato_do_mime(o) == Some(f))
                .map(String::as_str)
        })
}

/// Texto que veio do sistema: UTF-8 válido, quebras `\n`, sem o `\0` final que alguns apps
/// do Windows deixam.
pub fn texto_normalizado(bytes: &[u8]) -> Vec<u8> {
    let s = String::from_utf8_lossy(bytes);
    s.trim_end_matches('\0').replace("\r\n", "\n").into_bytes()
}

/// Caracteres que vão como estão numa URI `file://` (o resto vira `%XX`).
#[cfg(unix)]
fn sem_escape(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&b)
}

/// `text/uri-list` (ou `x-special/gnome-copied-files`) → caminhos locais. Só `file://` deste
/// computador; o resto é ignorado.
pub fn uris_para_caminhos(b: &[u8]) -> Vec<std::path::PathBuf> {
    String::from_utf8_lossy(b)
        .lines()
        .map(str::trim)
        .filter_map(|l| {
            let resto = l.strip_prefix("file://")?;
            let resto = resto.strip_prefix("localhost").unwrap_or(resto);
            resto.starts_with('/').then(|| decodificar(resto))?
        })
        .collect()
}

#[cfg(unix)]
fn decodificar(s: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(std::ffi::OsString::from_vec(out).into())
}

#[cfg(not(unix))]
fn decodificar(_: &str) -> Option<std::path::PathBuf> {
    None
}

/// Caminhos locais → o conteúdo de `mime` (`text/uri-list` ou `x-special/gnome-copied-files`).
#[cfg(unix)]
pub fn caminhos_para_uris(mime: &str, caminhos: &[std::path::PathBuf]) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let uris: Vec<String> = caminhos
        .iter()
        .map(|c| {
            let mut u = String::from("file://");
            for &b in c.as_os_str().as_bytes() {
                if sem_escape(b) {
                    u.push(b as char);
                } else {
                    u.push_str(&format!("%{b:02X}"));
                }
            }
            u
        })
        .collect();
    if mime == "x-special/gnome-copied-files" {
        format!("copy\n{}", uris.join("\n")).into_bytes()
    } else {
        let mut s = uris.join("\r\n");
        s.push_str("\r\n");
        s.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn uris_ida_e_volta() {
        use std::path::PathBuf;
        let c = vec![
            PathBuf::from("/home/ana/Área de trabalho/nota 1.txt"),
            PathBuf::from("/tmp/a#b%c"),
        ];
        let u = caminhos_para_uris("text/uri-list", &c);
        assert!(String::from_utf8_lossy(&u).starts_with("file:///home/ana/%C3%81rea%20de"));
        assert_eq!(uris_para_caminhos(&u), c);
        let g = caminhos_para_uris("x-special/gnome-copied-files", &c);
        assert!(g.starts_with(b"copy\nfile://"));
        assert_eq!(uris_para_caminhos(&g), c);
        assert_eq!(
            uris_para_caminhos(
                b"# comentario\r\nfile://localhost/x\r\nhttps://a/b\r\nfile://outro/y"
            ),
            vec![PathBuf::from("/x")]
        );
        assert!(uris_para_caminhos(b"file:///a%zz").is_empty());
    }

    fn v(m: &[&str]) -> Vec<String> {
        m.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mimes_viram_formatos() {
        let oferta = v(&[
            "text/html",
            "TEXT",
            "text/plain;charset=UTF-8",
            "application/x-qt-image",
            "image/png",
        ]);
        assert_eq!(
            formatos(&oferta),
            vec![ClipFormat::Text, ClipFormat::Html, ClipFormat::Png]
        );
        assert_eq!(
            formato_do_mime("text/plain;charset=UTF-8"),
            Some(ClipFormat::Text)
        );
        assert_eq!(formato_do_mime("image/jpeg"), None);
        assert_eq!(formato_do_mime("text/uri-list"), Some(ClipFormat::Files));
    }

    #[test]
    fn le_o_mime_preferido() {
        let oferta = v(&["TEXT", "UTF8_STRING", "text/plain;charset=UTF-8"]);
        // A grafia exata da lista vence; senão, qualquer um do mesmo formato.
        assert_eq!(
            mime_para_ler(ClipFormat::Text, &oferta),
            Some("UTF8_STRING")
        );
        let oferta = v(&["text/plain;charset=UTF-8"]);
        assert_eq!(
            mime_para_ler(ClipFormat::Text, &oferta),
            Some("text/plain;charset=UTF-8")
        );
        assert_eq!(mime_para_ler(ClipFormat::Png, &oferta), None);
    }

    #[test]
    fn texto_sem_crlf_nem_nulo() {
        assert_eq!(texto_normalizado(b"a\r\nb\0"), b"a\nb");
        assert_eq!(texto_normalizado(&[0xff, b'x']), "\u{fffd}x".as_bytes());
    }
}
