//! Troca de arquivos no fio (`10-plano-windows-linux.md` §3).
//!
//! Comandos e metadados andam no stream de controle (faixa de tags 80–89 do `oneof`); os bytes
//! de cada arquivo vão num stream QUIC unidirecional próprio, que começa com o [`DataHeader`]
//! de 16 bytes e segue com o conteúdo cru a partir de `offset`.
//!
//! Fluxo de uma transferência (igual nos dois sentidos):
//! 1. quem envia oferece o arquivo (no download, depois do pedido de quem baixa);
//! 2. quem recebe aceita com o `offset` (tamanho do `.part`, 0 se novo) e o BLAKE3 desse prefixo;
//! 3. quem envia confere o prefixo, abre o stream de dados no `offset` e, no fim, manda o
//!    BLAKE3 do arquivo inteiro para quem recebe conferir.

/// Limites da troca de arquivos.
pub mod limits {
    /// Caminho relativo, em bytes UTF-8.
    pub const PATH_LEN: usize = 4096;
    /// Nome de um item de listagem, em bytes UTF-8.
    pub const NAME_LEN: usize = 1024;
    /// Itens por mensagem de listagem; pastas maiores vêm em páginas.
    pub const LISTING_ENTRIES: usize = 2048;
    /// Bloco de leitura e escrita do stream de dados.
    pub const CHUNK: usize = 256 * 1024;
    /// Transferências simultâneas por sessão.
    pub const PARALLEL: usize = 4;
}

/// Raiz `0` dos comandos: a pasta de entrada de quem recebe (`~/Downloads/TailDesk`).
pub const INBOX_ROOT: u32 = 0;

/// BLAKE3 de 32 bytes.
pub type Hash = [u8; 32];

/// Cabeçalho do stream de dados de um arquivo: `"TDF"`, versão 1, `xfer_id`, `offset`
/// (little-endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataHeader {
    pub xfer_id: u32,
    /// Posição no arquivo do primeiro byte que segue o cabeçalho.
    pub offset: u64,
}

impl DataHeader {
    pub const LEN: usize = 16;
    const MAGIC: [u8; 4] = *b"TDF\x01";

    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0; Self::LEN];
        out[..4].copy_from_slice(&Self::MAGIC);
        out[4..8].copy_from_slice(&self.xfer_id.to_le_bytes());
        out[8..].copy_from_slice(&self.offset.to_le_bytes());
        out
    }

    /// `None` se não começar com `"TDF"` versão 1.
    pub fn decode(buf: &[u8; Self::LEN]) -> Option<DataHeader> {
        let (magic, rest) = buf.split_first_chunk::<4>()?;
        let (xfer_id, offset) = rest.split_first_chunk::<4>()?;
        if *magic != Self::MAGIC {
            return None;
        }
        Some(DataHeader {
            xfer_id: u32::from_le_bytes(*xfer_id),
            offset: u64::from_le_bytes(offset.try_into().ok()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_header_round_trip() {
        let h = DataHeader {
            xfer_id: 0xdead_beef,
            offset: 10 * 1024 * 1024 * 1024 + 5,
        };
        let buf = h.encode();
        assert_eq!(&buf[..4], b"TDF\x01");
        assert_eq!(&buf[4..8], &[0xef, 0xbe, 0xad, 0xde]);
        assert_eq!(DataHeader::decode(&buf), Some(h));
    }

    #[test]
    fn data_header_rejects_other_magic_or_version() {
        let buf = DataHeader {
            xfer_id: 1,
            offset: 0,
        }
        .encode();
        let mut v2 = buf;
        v2[3] = 2;
        assert_eq!(DataHeader::decode(&v2), None);
        let mut other = buf;
        other[0] = b'X';
        assert_eq!(DataHeader::decode(&other), None);
    }
}
