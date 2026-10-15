//! Mouse no Linux pelo portal RemoteDesktop, na mesma sessão da captura ([`SessaoPortal`]).
//! O monitor de cada `display_id` e a escala para pixels lógicos ficam com a sessão.

use super::EventoMouse;
use crate::wayland::{ErroPortal, SessaoPortal};

/// Injeta um evento. Assíncrono (D-Bus): rode na tarefa de input da sessão.
pub async fn injetar(sessao: &SessaoPortal, ev: &EventoMouse) -> Result<(), ErroPortal> {
    match *ev {
        EventoMouse::Absoluto { display_id, x, y } => {
            sessao.ponteiro_absoluto(display_id, x, y).await
        }
        EventoMouse::Relativo { dx, dy } => {
            sessao.ponteiro_relativo(f64::from(dx), f64::from(dy)).await
        }
        EventoMouse::Botao { botao, down } => sessao.botao(botao.evdev(), down).await,
        EventoMouse::Rolagem {
            dx,
            dy,
            precisa: true,
        } => {
            sessao
                .eixo(f64::from(dx), f64::from(dy), dx == 0 && dy == 0)
                .await
        }
        EventoMouse::Rolagem {
            dx,
            dy,
            precisa: false,
        } => {
            if dy != 0 {
                sessao.eixo_discreto(false, dy).await?;
            }
            if dx != 0 {
                sessao.eixo_discreto(true, dx).await?;
            }
            Ok(())
        }
    }
}
