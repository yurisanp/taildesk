//! Registro local de auditoria das transferências (`10-plano-windows-linux.md` §3, Segurança):
//! uma linha JSON por arquivo enviado ou recebido, com quem, o quê, tamanho, hash e resultado.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use taildesk_proto::files::Hash;

use crate::xfer::to_unix_ms;

/// Sentido da transferência, visto deste computador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// O peer mandou para cá.
    In,
    /// Daqui para o peer.
    Out,
}

/// Uma linha do registro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record<'a> {
    /// Identidade do peer vinda de `Overlay::identify()` (usuário e máquina da tailnet).
    pub peer: &'a str,
    pub direction: Direction,
    /// Caminho local do arquivo.
    pub path: &'a Path,
    pub size: u64,
    /// BLAKE3 conferido; `None` se a transferência falhou antes.
    pub blake3: Option<Hash>,
    /// `"ok"` ou o nome do `FileErrorCode` (`"HASH_MISMATCH"`...).
    pub result: &'a str,
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl Record<'_> {
    /// A linha JSON, sem o `\n`.
    pub fn to_json(&self, ts_ms: i64) -> String {
        let mut out = String::with_capacity(256);
        let _ = write!(out, "{{\"ts_ms\":{ts_ms},\"peer\":");
        json_str(&mut out, self.peer);
        out.push_str(",\"dir\":");
        out.push_str(match self.direction {
            Direction::In => "\"in\"",
            Direction::Out => "\"out\"",
        });
        out.push_str(",\"path\":");
        json_str(&mut out, &self.path.to_string_lossy());
        let _ = write!(out, ",\"size\":{},\"blake3\":", self.size);
        match &self.blake3 {
            Some(h) => {
                out.push('"');
                for b in h {
                    let _ = write!(out, "{b:02x}");
                }
                out.push('"');
            }
            None => out.push_str("null"),
        }
        out.push_str(",\"result\":");
        json_str(&mut out, self.result);
        out.push('}');
        out
    }
}

/// Arquivo de registro (`.jsonl`), só acrescentado.
#[derive(Clone, Debug)]
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        AuditLog { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Acrescenta uma linha. Cria o arquivo e as pastas se preciso.
    pub fn append(&self, record: &Record<'_>) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut line = record.to_json(to_unix_ms(Ok(SystemTime::now())));
        line.push('\n');
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(line.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_line_escapes_and_formats() {
        let r = Record {
            peer: "yuri@\"casa\"",
            direction: Direction::In,
            path: Path::new("a\\b\n.txt"),
            size: 3,
            blake3: Some([0xab; 32]),
            result: "ok",
        };
        let line = r.to_json(7);
        assert_eq!(
            line,
            format!(
                "{{\"ts_ms\":7,\"peer\":\"yuri@\\\"casa\\\"\",\"dir\":\"in\",\"path\":\"a\\\\b\\n.txt\",\"size\":3,\"blake3\":\"{}\",\"result\":\"ok\"}}",
                "ab".repeat(32)
            )
        );
        let failed = Record {
            blake3: None,
            direction: Direction::Out,
            result: "HASH_MISMATCH",
            ..r
        };
        assert!(failed.to_json(1).contains("\"dir\":\"out\""));
        assert!(failed.to_json(1).contains("\"blake3\":null"));
        assert!(!failed.to_json(1).contains('\n'));
        assert!(
            Record {
                peer: "\u{1}",
                ..failed
            }
            .to_json(1)
            .contains("\\u0001")
        );
    }
}
