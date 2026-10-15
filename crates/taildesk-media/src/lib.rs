//! Pipeline do host (captura, encode, packetização, pacing) e do controlador (remontagem, jitter buffer, decode).
//!
//! Esqueleto: implementação nas fases descritas em `docs/context/10-plano-windows-linux.md`.
//! Já existem as partes puras da seleção adaptativa (`docs/context/14-selecao-adaptativa.md`):
//! - [`measure`]: estatísticas de RTT e sondagem de banda;
//! - [`adapt`]: classe de rede, perfil de mídia e troca de classe com histerese.

pub mod adapt;
pub mod measure;
