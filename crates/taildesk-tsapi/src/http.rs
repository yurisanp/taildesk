//! Leitura mínima de uma resposta HTTP/1.1 completa (`Content-Length`, `chunked` ou até o fim).

use std::io;

fn bad(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Separa o código de status e o corpo de uma resposta já lida por inteiro.
pub fn parse_response(raw: &[u8]) -> io::Result<(u16, Vec<u8>)> {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(bad("resposta HTTP sem fim de cabeçalho"))?;
    let head = std::str::from_utf8(&raw[..head_end]).map_err(|_| bad("cabeçalho não é UTF-8"))?;
    let body = &raw[head_end + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(bad("resposta vazia"))?;
    let mut parts = status_line.splitn(3, ' ');
    if !parts.next().is_some_and(|v| v.starts_with("HTTP/1.")) {
        return Err(bad("não é HTTP/1.x"));
    }
    let status: u16 = parts
        .next()
        .and_then(|c| c.parse().ok())
        .ok_or(bad("código de status inválido"))?;

    let mut chunked = false;
    let mut length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case("chunked");
        } else if name.eq_ignore_ascii_case("content-length") {
            length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| bad("Content-Length inválido"))?,
            );
        }
    }

    let body = if chunked {
        dechunk(body)?
    } else if let Some(n) = length {
        body.get(..n)
            .ok_or(bad("corpo menor que o Content-Length"))?
            .to_vec()
    } else {
        body.to_vec()
    };
    Ok((status, body))
}

fn dechunk(mut data: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or(bad("chunk sem tamanho"))?;
        let size_str =
            std::str::from_utf8(&data[..eol]).map_err(|_| bad("tamanho de chunk inválido"))?;
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let size =
            usize::from_str_radix(size_hex, 16).map_err(|_| bad("tamanho de chunk inválido"))?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        let chunk = data.get(..size).ok_or(bad("chunk truncado"))?;
        out.extend_from_slice(chunk);
        data = data.get(size + 2..).ok_or(bad("chunk truncado"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_length_and_close() {
        let raw =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(parse_response(raw).unwrap(), (200, b"{}".to_vec()));
        let raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n[1,2]";
        assert_eq!(parse_response(raw).unwrap(), (200, b"[1,2]".to_vec()));
    }

    #[test]
    fn chunked_body() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3;x=y\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap(), (200, b"{\"a\":1}".to_vec()));
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_response(b"HTTP/1.1 200 OK\r\n").is_err());
        assert!(parse_response(b"SSH-2.0\r\n\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 abc OK\r\n\r\n").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n{}").is_err());
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nff\r\nab")
                .is_err()
        );
    }
}
