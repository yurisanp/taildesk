//! Pipeline do host (captura, encode, packetização, pacing) e do controlador (remontagem, jitter buffer, decode).
//!
//! - [`video`]: envio e recepção de vídeo por monitor (encode, fragmentação, remontagem, decode);
//! - [`audio`]: áudio do sistema em Opus (conversão, silêncio, PLC e fila de reprodução).
//!
//! Também existem as partes puras da seleção adaptativa (`docs/context/14-selecao-adaptativa.md`):
//! - [`measure`]: estatísticas de RTT e sondagem de banda;
//! - [`adapt`]: classe de rede, perfil de mídia e troca de classe com histerese;
//! - [`taxa`]: laço rápido do bitrate (fila, perda e banda do QUIC);
//! - [`escada`]: laço lento de resolução e FPS (degraus com histerese).

pub mod adapt;
pub mod audio;
pub mod escada;
pub mod measure;
pub mod taxa;
pub mod video;
