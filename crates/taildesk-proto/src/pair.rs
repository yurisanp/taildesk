//! Pareamento de um celular com um desktop (`mobile/android-detalhado.md` §6–7, MF-20..22).
//!
//! O desktop mostra um QR com uma [`PairOffer`] em forma de URI:
//!
//! ```text
//! taildesk://pair?v=1&n=<nome>&a=<IP da tailnet>&p=<porta>&f=<SHA-256 do certificado>&c=<código>
//! ```
//!
//! O celular conecta em `a:p`, confere que o certificado do host bate com `f` e manda
//! `PairRequest{code: c}` numa conexão de serviço. O código vale uma vez, por
//! [`CODE_TTL_SECS`]; sem câmera, a pessoa digita o nome do computador e o código.
//! Quem o celular é continua vindo do `whois` do host, nunca daqui.

use std::fmt;
use std::net::IpAddr;

/// Esquema e caminho da URI do QR.
pub const URI_PREFIX: &str = "taildesk://pair?";
/// Versão do formato da URI.
pub const URI_VERSION: u32 = 1;
/// Tamanho do código normalizado.
pub const CODE_LEN: usize = 6;
/// Base32 de Crockford: números e letras sem I, L, O e U (que se confundem ou formam palavras).
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Validade de um código, em segundos (5 minutos).
pub const CODE_TTL_SECS: u64 = 5 * 60;
/// Maior URI aceita (cabe num QR versão 10 com folga).
pub const URI_MAX_LEN: usize = 512;

/// O que o QR do desktop carrega.
#[derive(Clone, PartialEq, Eq)]
pub struct PairOffer {
    /// Nome do computador, só para exibir (≤ `control::limits::SHORT_TEXT_LEN` bytes).
    pub host_name: String,
    /// IP do desktop na tailnet.
    pub addr: IpAddr,
    pub port: u16,
    /// SHA-256 do certificado QUIC do host.
    pub fingerprint: [u8; 32],
    /// Código de uso único, normalizado ([`normalize_code`]).
    pub code: String,
}

impl fmt::Debug for PairOffer {
    // O código é um segredo de curta duração: não vai para logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairOffer")
            .field("host_name", &self.host_name)
            .field("addr", &self.addr)
            .field("port", &self.port)
            .field("fingerprint", &hex(&self.fingerprint))
            .field("code", &"***")
            .finish()
    }
}

/// Erro ao ler a URI de um QR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairUriError {
    /// Não é uma URI de pareamento do TailDesk.
    NotTaildesk,
    /// Versão mais nova que esta: atualizar o app.
    Version,
    TooLong,
    /// Falta o campo ou ele está malformado.
    Field(&'static str),
}

impl fmt::Display for PairUriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PairUriError::NotTaildesk => f.write_str("não é um QR do TailDesk"),
            PairUriError::Version => f.write_str("QR de uma versão mais nova do TailDesk"),
            PairUriError::TooLong => f.write_str("QR grande demais"),
            PairUriError::Field(c) => write!(f, "campo {c} inválido no QR"),
        }
    }
}

impl std::error::Error for PairUriError {}

/// Gera um código a partir de bytes aleatórios (quem chama usa o gerador do sistema).
/// Cada byte escolhe um de 32 símbolos, então não há viés.
pub fn code_from_random(random: [u8; CODE_LEN]) -> String {
    random
        .iter()
        .map(|b| char::from(CODE_ALPHABET[usize::from(b & 31)]))
        .collect()
}

/// Normaliza o que a pessoa digitou: maiúsculas, sem espaços nem hífens, `O` vira `0` e `I`/`L`
/// viram `1` (como no Crockford). Devolve `None` se sobrar algo fora de [`CODE_ALPHABET`] ou o
/// tamanho não for [`CODE_LEN`].
pub fn normalize_code(typed: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for c in typed.chars() {
        if c == '-' || c == ' ' {
            continue;
        }
        let c = c.to_ascii_uppercase();
        let c = match c {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !CODE_ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
        if out.len() > CODE_LEN {
            return None;
        }
    }
    (out.len() == CODE_LEN).then_some(out)
}

/// Código em dois grupos de três para mostrar na tela ("K7P-2QX").
pub fn display_code(code: &str) -> String {
    if code.len() == CODE_LEN && code.is_ascii() {
        format!("{}-{}", &code[..3], &code[3..])
    } else {
        code.to_owned()
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// Codifica para a URI: letras, números e `-._~` passam; o resto vira `%XX` (UTF-8).
fn pct_encode(s: &str, out: &mut String) {
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
}

fn pct_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let h = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

impl PairOffer {
    /// A URI que vai no QR.
    pub fn to_uri(&self) -> String {
        let mut s = String::from(URI_PREFIX);
        s.push_str(&format!("v={URI_VERSION}&n="));
        pct_encode(&self.host_name, &mut s);
        s.push_str("&a=");
        pct_encode(&self.addr.to_string(), &mut s);
        s.push_str(&format!(
            "&p={}&f={}&c={}",
            self.port,
            hex(&self.fingerprint),
            self.code
        ));
        s
    }

    /// Lê a URI de um QR. Campos desconhecidos são ignorados (versões futuras podem somar).
    pub fn from_uri(uri: &str) -> Result<PairOffer, PairUriError> {
        if uri.len() > URI_MAX_LEN {
            return Err(PairUriError::TooLong);
        }
        let query = uri
            .strip_prefix(URI_PREFIX)
            .ok_or(PairUriError::NotTaildesk)?;
        let (mut v, mut n, mut a, mut p, mut f, mut c) = (None, None, None, None, None, None);
        for par in query.split('&') {
            let (k, val) = par.split_once('=').unwrap_or((par, ""));
            let val = pct_decode(val).ok_or(PairUriError::Field("encoding"))?;
            match k {
                "v" => v = Some(val),
                "n" => n = Some(val),
                "a" => a = Some(val),
                "p" => p = Some(val),
                "f" => f = Some(val),
                "c" => c = Some(val),
                _ => {}
            }
        }
        let v: u32 = v
            .and_then(|v| v.parse().ok())
            .ok_or(PairUriError::Field("v"))?;
        if v > URI_VERSION {
            return Err(PairUriError::Version);
        }
        let host_name = n.unwrap_or_default();
        if host_name.len() > crate::control::limits::SHORT_TEXT_LEN {
            return Err(PairUriError::Field("n"));
        }
        let addr = a
            .and_then(|a| a.parse().ok())
            .ok_or(PairUriError::Field("a"))?;
        let port = p
            .and_then(|p| p.parse().ok())
            .filter(|&p| p != 0)
            .ok_or(PairUriError::Field("p"))?;
        let fingerprint = f
            .and_then(|f| unhex32(&f.to_ascii_lowercase()))
            .ok_or(PairUriError::Field("f"))?;
        let code = c
            .and_then(|c| normalize_code(&c))
            .ok_or(PairUriError::Field("c"))?;
        Ok(PairOffer {
            host_name,
            addr,
            port,
            fingerprint,
            code,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oferta() -> PairOffer {
        PairOffer {
            host_name: "Notebook do Yuri & cia".into(),
            addr: "100.101.102.103".parse().unwrap(),
            port: 7420,
            fingerprint: [0xab; 32],
            code: "K7P2QX".into(),
        }
    }

    #[test]
    fn round_trip_da_uri() {
        for o in [
            oferta(),
            PairOffer {
                host_name: "pc-mãe 🖥".into(),
                addr: "fd7a:115c:a1e0::1".parse().unwrap(),
                port: 65535,
                fingerprint: core::array::from_fn(|i| i as u8),
                code: "ABCDEF".into(),
            },
        ] {
            let uri = o.to_uri();
            assert!(uri.starts_with(URI_PREFIX), "{uri}");
            assert!(uri.len() <= URI_MAX_LEN);
            assert_eq!(PairOffer::from_uri(&uri), Ok(o));
        }
    }

    #[test]
    fn uri_estranha_e_recusada() {
        let boa = oferta().to_uri();
        assert_eq!(
            PairOffer::from_uri("https://exemplo.com/?c=ABCDEF"),
            Err(PairUriError::NotTaildesk)
        );
        assert_eq!(
            PairOffer::from_uri(&boa.replace("v=1", "v=2")),
            Err(PairUriError::Version)
        );
        assert_eq!(
            PairOffer::from_uri(&boa.replace("p=7420", "p=0")),
            Err(PairUriError::Field("p"))
        );
        assert_eq!(
            PairOffer::from_uri(&boa.replace("a=100.101.102.103", "a=meu-pc")),
            Err(PairUriError::Field("a"))
        );
        assert_eq!(
            PairOffer::from_uri(&boa.replace(&"ab".repeat(32), &"ab".repeat(31))),
            Err(PairUriError::Field("f"))
        );
        assert_eq!(
            PairOffer::from_uri(&boa.replace("c=K7P2QX", "c=K7P")),
            Err(PairUriError::Field("c"))
        );
        assert_eq!(
            PairOffer::from_uri(&format!("{boa}&x={}", "y".repeat(URI_MAX_LEN))),
            Err(PairUriError::TooLong)
        );
        // Campo desconhecido de uma versão futura compatível é ignorado.
        assert!(PairOffer::from_uri(&format!("{boa}&novo=1")).is_ok());
    }

    #[test]
    fn codigo_digitado_e_normalizado() {
        assert_eq!(normalize_code("k7p-2qx").as_deref(), Some("K7P2QX"));
        assert_eq!(normalize_code(" K7P 2QX ").as_deref(), Some("K7P2QX"));
        // Símbolos que se confundem viram o parecido do alfabeto.
        assert_eq!(normalize_code("oIl-abc").as_deref(), Some("011ABC"));
        assert_eq!(normalize_code("UUUUUU"), None);
        assert_eq!(normalize_code("K7P2Q"), None);
        assert_eq!(normalize_code("K7P2QXA"), None);
        assert_eq!(normalize_code("K7P2Q!"), None);
        assert_eq!(normalize_code("K7P2Qç"), None);
        assert_eq!(display_code("K7P2QX"), "K7P-2QX");
    }

    #[test]
    fn codigo_gerado_usa_so_o_alfabeto() {
        for seed in 0..=255u8 {
            let c = code_from_random([seed, seed ^ 0x55, 255, 0, 31, 32]);
            assert_eq!(c.len(), CODE_LEN);
            assert_eq!(normalize_code(&c).as_deref(), Some(c.as_str()));
        }
    }

    #[test]
    fn debug_nao_mostra_o_codigo() {
        let d = format!("{:?}", oferta());
        assert!(!d.contains("K7P2QX") && d.contains("***"), "{d}");
    }
}
