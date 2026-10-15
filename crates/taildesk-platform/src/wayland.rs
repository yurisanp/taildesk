//! Sessão do portal no Linux (`org.freedesktop.portal.RemoteDesktop` com `ScreenCast` dentro).
//!
//! Uma sessão só para captura e input, com um diálogo só no host: `CreateSession`
//! (RemoteDesktop) → `SelectDevices(KEYBOARD | POINTER)` → `ScreenCast.SelectSources(MONITOR,
//! multiple)` → `Start`. O portal devolve um stream PipeWire por monitor escolhido pela pessoa
//! (`13-multimonitor.md` §4.2); cada um vira um `display_id` na ordem em que vieram.
//!
//! Onde o portal não tem RemoteDesktop (wlroots: Sway, Hyprland), a sessão sai só com
//! ScreenCast: a captura funciona, [`SessaoPortal::dispositivos`] vem vazio e
//! [`SessaoPortal::sem_input`] diz o motivo para a UI mostrar. Nada de contornar o portal.
//!
//! A injeção aqui é fina, sem lógica de layout (a tradução de teclado é da fase 2,
//! `11-teclado.md`). Funciona igual numa sessão X11 com portal (KDE, GNOME).

use std::fmt;
use std::os::fd::OwnedFd;

use ashpd::PortalError;
use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, NotifyKeyboardKeycodeOptions, NotifyKeyboardKeysymOptions,
    NotifyPointerAxisDiscreteOptions, NotifyPointerAxisOptions, NotifyPointerButtonOptions,
    NotifyPointerMotionAbsoluteOptions, NotifyPointerMotionOptions, RemoteDesktop,
    SelectDevicesOptions, StartOptions,
};
use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions, Stream,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, ResponseError, Session};
use ashpd::enumflags2::BitFlags;

/// Coordenada normalizada máxima do protocolo (`taildesk_proto::control::limits::COORD_MAX`).
const COORD_MAX: f64 = u16::MAX as f64;

/// Um monitor liberado pela pessoa no diálogo do portal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorPortal {
    /// Índice na sessão; é o `display_id` do protocolo.
    pub display_id: u32,
    /// Node do PipeWire com o vídeo desse monitor.
    pub node_id: u32,
    /// Posição no layout do compositor, em pixels lógicos.
    pub posicao: (i32, i32),
    /// Tamanho em pixels lógicos (o tamanho físico vem da negociação do PipeWire).
    pub tamanho_logico: (u32, u32),
    /// Liga o stream à região do libei (`mapping_id`, RemoteDesktop v2); vazio se o portal não
    /// informa.
    pub mapping_id: String,
}

impl MonitorPortal {
    fn novo(
        display_id: u32,
        node_id: u32,
        posicao: Option<(i32, i32)>,
        tamanho: Option<(i32, i32)>,
        mapping_id: Option<&str>,
    ) -> MonitorPortal {
        let (w, h) = tamanho.unwrap_or_default();
        MonitorPortal {
            display_id,
            node_id,
            posicao: posicao.unwrap_or_default(),
            tamanho_logico: (w.max(0) as u32, h.max(0) as u32),
            mapping_id: mapping_id.unwrap_or_default().to_owned(),
        }
    }

    /// Converte `x`, `y` normalizados do protocolo (0..=65535 dentro do monitor) para o espaço
    /// lógico do stream, que é o que `NotifyPointerMotionAbsolute` espera.
    pub fn para_logico(&self, x: u16, y: u16) -> (f64, f64) {
        let escala = |v: u16, tam: u32| f64::from(v) / COORD_MAX * f64::from(tam.saturating_sub(1));
        (
            escala(x, self.tamanho_logico.0),
            escala(y, self.tamanho_logico.1),
        )
    }
}

/// Dispositivos de input que o compositor concedeu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dispositivos {
    pub teclado: bool,
    pub ponteiro: bool,
}

/// Por que não há input nesta sessão.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemInput {
    /// O portal do compositor não implementa RemoteDesktop (wlroots: Sway, Hyprland).
    SemRemoteDesktop,
    /// A pessoa no host não liberou teclado nem mouse no diálogo.
    NaoLiberado,
}

/// Falha ao abrir a sessão.
#[derive(Debug)]
pub enum ErroPortal {
    /// Sem `xdg-desktop-portal` ou sem ScreenCast no backend do compositor.
    SemPortal,
    /// A pessoa no host cancelou o diálogo.
    Cancelado,
    /// O diálogo terminou sem nenhum monitor.
    SemMonitores,
    /// Monitor fora da sessão.
    MonitorDesconhecido(u32),
    /// A operação precisa de input, mas a sessão não tem.
    SemInput(SemInput),
    Portal(ashpd::Error),
}

impl fmt::Display for ErroPortal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErroPortal::SemPortal => f.write_str(
                "a sessão não tem o portal de captura de tela (xdg-desktop-portal com o backend do \
                 compositor)",
            ),
            ErroPortal::Cancelado => f.write_str("o compartilhamento foi recusado no host"),
            ErroPortal::SemMonitores => f.write_str("nenhum monitor foi liberado no host"),
            ErroPortal::MonitorDesconhecido(id) => write!(f, "monitor {id} fora da sessão"),
            ErroPortal::SemInput(SemInput::SemRemoteDesktop) => f.write_str(
                "o portal deste compositor não permite controle remoto (só visualização)",
            ),
            ErroPortal::SemInput(SemInput::NaoLiberado) => {
                f.write_str("teclado e mouse não foram liberados no host")
            }
            ErroPortal::Portal(e) => write!(f, "portal: {e}"),
        }
    }
}

impl std::error::Error for ErroPortal {}

/// Sem barramento de sessão ou sem `xdg-desktop-portal` registrado nele.
fn sem_portal(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::Address(_) | zbus::Error::InputOutput(_) | zbus::Error::Connection(..) => true,
        zbus::Error::MethodError(nome, ..) => {
            nome.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown"
        }
        _ => false,
    }
}

impl From<ashpd::Error> for ErroPortal {
    fn from(e: ashpd::Error) -> ErroPortal {
        match e {
            ashpd::Error::Response(ResponseError::Cancelled) => ErroPortal::Cancelado,
            ashpd::Error::PortalNotFound(_) => ErroPortal::SemPortal,
            ashpd::Error::Zbus(ref z) | ashpd::Error::Portal(PortalError::ZBus(ref z))
                if sem_portal(z) =>
            {
                ErroPortal::SemPortal
            }
            e => ErroPortal::Portal(e),
        }
    }
}

enum Tipo {
    Remota {
        rd: RemoteDesktop,
        sessao: Session<RemoteDesktop>,
    },
    SoTela {
        sessao: Session<Screencast>,
    },
}

/// Sessão aberta do portal: captura dos monitores liberados e input na mesma sessão.
///
/// Fechar (ou soltar) a sessão encerra captura e input juntos.
pub struct SessaoPortal {
    tipo: Tipo,
    screencast: Screencast,
    monitores: Vec<MonitorPortal>,
    dispositivos: Dispositivos,
    sem_input: Option<SemInput>,
}

impl fmt::Debug for SessaoPortal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessaoPortal")
            .field("remote_desktop", &matches!(self.tipo, Tipo::Remota { .. }))
            .field("monitores", &self.monitores)
            .field("dispositivos", &self.dispositivos)
            .field("sem_input", &self.sem_input)
            .finish()
    }
}

impl SessaoPortal {
    /// Abre a sessão e mostra o diálogo do compositor no host. Espera a pessoa responder.
    ///
    /// `com_input = false` pede só a tela (sessão só de visualização).
    pub async fn abrir(com_input: bool) -> Result<SessaoPortal, ErroPortal> {
        let screencast = Screencast::new().await?;
        let fontes = SelectSourcesOptions::default()
            .set_sources(BitFlags::from(SourceType::Monitor))
            .set_multiple(true)
            .set_cursor_mode(modo_cursor(&screencast).await)
            .set_persist_mode(PersistMode::DoNot);

        if com_input {
            match RemoteDesktop::new().await {
                Ok(rd) => return Self::abrir_remota(rd, screencast, fontes).await,
                Err(ashpd::Error::PortalNotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }

        let sessao = screencast
            .create_session(CreateSessionOptions::default())
            .await?;
        screencast
            .select_sources(&sessao, fontes)
            .await?
            .response()?;
        let streams = screencast
            .start(&sessao, None, StartCastOptions::default())
            .await?
            .response()?;
        let monitores = monitores(streams.streams())?;
        Ok(SessaoPortal {
            tipo: Tipo::SoTela { sessao },
            screencast,
            monitores,
            dispositivos: Dispositivos::default(),
            sem_input: com_input.then_some(SemInput::SemRemoteDesktop),
        })
    }

    async fn abrir_remota(
        rd: RemoteDesktop,
        screencast: Screencast,
        fontes: SelectSourcesOptions,
    ) -> Result<SessaoPortal, ErroPortal> {
        let sessao = rd.create_session(CreateSessionOptions::default()).await?;
        rd.select_devices(
            &sessao,
            SelectDevicesOptions::default()
                .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await?
        .response()?;
        screencast
            .select_sources(&sessao, fontes)
            .await?
            .response()?;
        let escolha = rd
            .start(&sessao, None, StartOptions::default())
            .await?
            .response()?;
        let monitores = monitores(escolha.streams())?;
        let concedidos = escolha.devices();
        let dispositivos = Dispositivos {
            teclado: concedidos.contains(DeviceType::Keyboard),
            ponteiro: concedidos.contains(DeviceType::Pointer),
        };
        let sem_input =
            (!dispositivos.teclado && !dispositivos.ponteiro).then_some(SemInput::NaoLiberado);
        Ok(SessaoPortal {
            tipo: Tipo::Remota { rd, sessao },
            screencast,
            monitores,
            dispositivos,
            sem_input,
        })
    }

    /// Monitores liberados, na ordem do `display_id`.
    pub fn monitores(&self) -> &[MonitorPortal] {
        &self.monitores
    }

    pub fn monitor(&self, display_id: u32) -> Result<&MonitorPortal, ErroPortal> {
        self.monitores
            .get(display_id as usize)
            .ok_or(ErroPortal::MonitorDesconhecido(display_id))
    }

    pub fn dispositivos(&self) -> Dispositivos {
        self.dispositivos
    }

    /// `Some` quando a sessão é só de visualização, com o motivo para a UI.
    pub fn sem_input(&self) -> Option<SemInput> {
        self.sem_input
    }

    /// Conexão com o PipeWire que enxerga só os streams desta sessão.
    pub async fn abrir_pipewire(&self) -> Result<OwnedFd, ErroPortal> {
        let opcoes = OpenPipeWireRemoteOptions::default();
        Ok(match &self.tipo {
            Tipo::Remota { sessao, .. } => {
                self.screencast
                    .open_pipe_wire_remote(sessao, opcoes)
                    .await?
            }
            Tipo::SoTela { sessao } => {
                self.screencast
                    .open_pipe_wire_remote(sessao, opcoes)
                    .await?
            }
        })
    }

    /// Socket do EIS para injetar via libei (RemoteDesktop v2). `None` se o portal não tem.
    pub async fn conectar_eis(&self) -> Result<Option<OwnedFd>, ErroPortal> {
        let (rd, sessao) = self.remota()?;
        match rd.connect_to_eis(sessao, Default::default()).await {
            Ok(fd) => Ok(Some(fd)),
            Err(ashpd::Error::RequiresVersion(..)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Ponteiro em posição absoluta dentro de um monitor, com `x`, `y` normalizados do
    /// protocolo (0..=65535).
    pub async fn ponteiro_absoluto(
        &self,
        display_id: u32,
        x: u16,
        y: u16,
    ) -> Result<(), ErroPortal> {
        let (rd, sessao) = self.remota()?;
        let m = self.monitor(display_id)?;
        let (lx, ly) = m.para_logico(x, y);
        rd.notify_pointer_motion_absolute(
            sessao,
            m.node_id,
            lx,
            ly,
            NotifyPointerMotionAbsoluteOptions::default(),
        )
        .await?;
        Ok(())
    }

    /// Movimento relativo, em pixels lógicos.
    pub async fn ponteiro_relativo(&self, dx: f64, dy: f64) -> Result<(), ErroPortal> {
        let (rd, sessao) = self.remota()?;
        rd.notify_pointer_motion(sessao, dx, dy, NotifyPointerMotionOptions::default())
            .await?;
        Ok(())
    }

    /// Botão do mouse pelo código evdev (`BTN_LEFT` = 0x110, `BTN_RIGHT` = 0x111, …).
    pub async fn botao(&self, evdev: i32, pressionado: bool) -> Result<(), ErroPortal> {
        let (rd, sessao) = self.remota()?;
        rd.notify_pointer_button(
            sessao,
            evdev,
            estado(pressionado),
            NotifyPointerButtonOptions::default(),
        )
        .await?;
        Ok(())
    }

    /// Rolagem suave (touchpad), em pixels lógicos; `fim` encerra o gesto.
    pub async fn eixo(&self, dx: f64, dy: f64, fim: bool) -> Result<(), ErroPortal> {
        let (rd, sessao) = self.remota()?;
        rd.notify_pointer_axis(
            sessao,
            dx,
            dy,
            NotifyPointerAxisOptions::default().set_finish(fim),
        )
        .await?;
        Ok(())
    }

    /// Rolagem em dentes de roda; positivo = para baixo / para a direita.
    pub async fn eixo_discreto(&self, horizontal: bool, passos: i32) -> Result<(), ErroPortal> {
        let (rd, sessao) = self.remota()?;
        let eixo = if horizontal {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        rd.notify_pointer_axis_discrete(
            sessao,
            eixo,
            passos,
            NotifyPointerAxisDiscreteOptions::default(),
        )
        .await?;
        Ok(())
    }

    /// Tecla física pelo código evdev (`KEY_A` = 30), no layout ativo do host.
    pub async fn tecla_keycode(&self, evdev: i32, pressionada: bool) -> Result<(), ErroPortal> {
        let (rd, sessao) = self.remota()?;
        rd.notify_keyboard_keycode(
            sessao,
            evdev,
            estado(pressionada),
            NotifyKeyboardKeycodeOptions::default(),
        )
        .await?;
        Ok(())
    }

    /// Tecla pelo keysym do X (`XK_ccedilla` = 0xe7), quando não há tecla física no layout do
    /// host (`11-teclado.md` §2.4).
    pub async fn tecla_keysym(&self, keysym: i32, pressionada: bool) -> Result<(), ErroPortal> {
        let (rd, sessao) = self.remota()?;
        rd.notify_keyboard_keysym(
            sessao,
            keysym,
            estado(pressionada),
            NotifyKeyboardKeysymOptions::default(),
        )
        .await?;
        Ok(())
    }

    /// Espera o compositor ou a pessoa no host encerrar a sessão.
    pub async fn esperar_fechamento(&self) -> Result<(), ErroPortal> {
        use futures_util::StreamExt;
        match &self.tipo {
            Tipo::Remota { sessao, .. } => {
                sessao.receive_closed().await?.next().await;
            }
            Tipo::SoTela { sessao } => {
                sessao.receive_closed().await?.next().await;
            }
        }
        Ok(())
    }

    /// Encerra captura e input.
    pub async fn fechar(self) -> Result<(), ErroPortal> {
        match &self.tipo {
            Tipo::Remota { sessao, .. } => sessao.close().await?,
            Tipo::SoTela { sessao } => sessao.close().await?,
        }
        Ok(())
    }

    fn remota(&self) -> Result<(&RemoteDesktop, &Session<RemoteDesktop>), ErroPortal> {
        match (&self.tipo, self.sem_input) {
            (_, Some(motivo)) => Err(ErroPortal::SemInput(motivo)),
            (Tipo::Remota { rd, sessao }, None) => Ok((rd, sessao)),
            (Tipo::SoTela { .. }, None) => Err(ErroPortal::SemInput(SemInput::SemRemoteDesktop)),
        }
    }
}

/// Cursor como metadado do PipeWire (o controlador desenha o cursor por cima); embutido no
/// vídeo se o portal não oferece.
async fn modo_cursor(screencast: &Screencast) -> CursorMode {
    match screencast.available_cursor_modes().await {
        Ok(modos) if modos.contains(CursorMode::Metadata) => CursorMode::Metadata,
        _ => CursorMode::Embedded,
    }
}

fn monitores(streams: &[Stream]) -> Result<Vec<MonitorPortal>, ErroPortal> {
    numerar(streams.iter().map(|s| {
        (
            s.pipe_wire_node_id(),
            s.position(),
            s.size(),
            s.mapping_id(),
        )
    }))
}

type Bruto<'a> = (u32, Option<(i32, i32)>, Option<(i32, i32)>, Option<&'a str>);

/// `display_id` = ordem em que o portal devolveu os streams.
fn numerar<'a>(streams: impl Iterator<Item = Bruto<'a>>) -> Result<Vec<MonitorPortal>, ErroPortal> {
    let ms: Vec<MonitorPortal> = streams
        .enumerate()
        .map(|(i, (node, pos, tam, map))| MonitorPortal::novo(i as u32, node, pos, tam, map))
        .collect();
    if ms.is_empty() {
        Err(ErroPortal::SemMonitores)
    } else {
        Ok(ms)
    }
}

fn estado(pressionado: bool) -> KeyState {
    if pressionado {
        KeyState::Pressed
    } else {
        KeyState::Released
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_viram_monitores_na_ordem() {
        let ms = numerar(
            [
                (57, Some((0, 0)), Some((2048, 1152)), Some("eDP-1")),
                (61, Some((-1920, 120)), Some((1920, 1080)), None),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].display_id, 0);
        assert_eq!(ms[0].node_id, 57);
        assert_eq!(ms[1].display_id, 1);
        assert_eq!(ms[1].posicao, (-1920, 120));
        assert_eq!(ms[1].tamanho_logico, (1920, 1080));
        assert_eq!(ms[0].mapping_id, "eDP-1");
        assert!(ms[1].mapping_id.is_empty());
    }

    #[test]
    fn sem_streams_e_erro() {
        assert!(matches!(monitores(&[]), Err(ErroPortal::SemMonitores)));
        let m = numerar([(9, None, None, None)].into_iter()).unwrap();
        assert_eq!(m[0].tamanho_logico, (0, 0));
    }

    #[test]
    fn coordenadas_normalizadas_cobrem_o_monitor() {
        let m = MonitorPortal::novo(0, 1, Some((0, 0)), Some((1920, 1080)), None);
        assert_eq!(m.para_logico(0, 0), (0.0, 0.0));
        assert_eq!(m.para_logico(u16::MAX, u16::MAX), (1919.0, 1079.0));
        let (x, y) = m.para_logico(32_768, 32_768);
        assert!((x - 959.5).abs() < 0.1 && (y - 539.5).abs() < 0.1);
    }

    #[test]
    fn cancelar_vira_erro_proprio() {
        let e = ErroPortal::from(ashpd::Error::Response(ResponseError::Cancelled));
        assert!(matches!(e, ErroPortal::Cancelado));
    }
}
