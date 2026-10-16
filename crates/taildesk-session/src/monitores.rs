//! Monitores no controlador (`13-multimonitor.md` §2.1–§2.2, `12-levantamento-ux.md` §5.6.1):
//! abas, atalhos, "seguir o mouse", abas destacadas em janela própria e o mapeamento do mouse
//! local para o monitor.
//!
//! A janela da sessão mostra um monitor por vez: trocar é `StopVideo` do anterior + `StartVideo`
//! do novo (o host começa com keyframe). Uma aba **destacada** abre numa janela própria com o seu
//! stream, ao mesmo tempo que o da janela da sessão; destacar o monitor que a sessão mostra não
//! reinicia o vídeo, só muda a janela. Clicar na aba de um destacado o traz de volta.
//! Monitores fora da seleção do portal (Wayland) viram aba "não liberado".

use taildesk_keyboard::{Hid, Mods};
use taildesk_proto::control::{
    CursorPos, DisplayInfo, Msg, Priority, StartVideo, StopVideo, VideoStopped,
};

/// Uma aba da faixa de monitores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aba {
    pub display_id: u32,
    /// Número mostrado e usado no atalho `Ctrl+Alt+N` (1 = mais à esquerda no host).
    pub numero: u32,
    /// "DELL U2720Q · 2560×1440" (sem o número).
    pub rotulo: String,
    pub principal: bool,
    /// `false` = o host não liberou (Wayland): aba cinza com "Pedir acesso".
    pub liberado: bool,
    pub ativa: bool,
    /// Aberto numa janela própria.
    pub destacada: bool,
}

/// Ação de um atalho da sessão.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Atalho {
    Anterior,
    Proximo,
    /// `Ctrl+Alt+1…9`, número da aba.
    Direto(u32),
}

/// `Ctrl+Alt+←/→` e `Ctrl+Alt+1…9` (sem Shift, Meta ou AltGr). O controlador consome a tecla
/// em vez de mandá-la ao host.
pub fn atalho(hid: Hid, mods: Mods) -> Option<Atalho> {
    if mods.bits() != Mods::CTRL.bits() | Mods::ALT.bits() {
        return None;
    }
    match hid {
        0x50 => Some(Atalho::Anterior),
        0x4F => Some(Atalho::Proximo),
        0x1E..=0x26 => Some(Atalho::Direto(u32::from(hid - 0x1E) + 1)),
        _ => None,
    }
}

/// Estado do modo um por vez.
#[derive(Debug, Default)]
pub struct UmPorVez {
    /// Na ordem das abas: esquerda → direita, depois de cima para baixo, no layout do host.
    monitores: Vec<DisplayInfo>,
    ativo: Option<u32>,
    /// Monitores em janela própria, na ordem em que foram destacados.
    destacados: Vec<u32>,
    /// "Seguir o mouse" (desligado por padrão).
    pub seguir_mouse: bool,
}

impl UmPorVez {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ativo(&self) -> Option<u32> {
        self.ativo
    }

    pub fn monitor_ativo(&self) -> Option<&DisplayInfo> {
        let a = self.ativo?;
        self.monitores.iter().find(|m| m.id == a)
    }

    /// A faixa de abas só aparece com 2 ou mais monitores no host.
    pub fn mostrar_abas(&self) -> bool {
        self.monitores.len() >= 2
    }

    pub fn abas(&self) -> Vec<Aba> {
        self.monitores
            .iter()
            .zip(1..)
            .map(|(m, numero)| {
                let tamanho = format!("{}×{}", m.width, m.height);
                Aba {
                    display_id: m.id,
                    numero,
                    rotulo: if m.name.is_empty() {
                        tamanho
                    } else {
                        format!("{} · {tamanho}", m.name)
                    },
                    principal: m.primary,
                    liberado: m.shared,
                    ativa: self.ativo == Some(m.id),
                    destacada: self.destacados.contains(&m.id),
                }
            })
            .collect()
    }

    /// Lista do `HelloAck` ou de um `DisplaysChanged`. Se o ativo sumiu ou deixou de ser
    /// liberado, cai no principal (ou no primeiro liberado). Na primeira lista, abre o principal.
    pub fn atualizar(&mut self, displays: &[DisplayInfo], out: &mut Vec<Msg>) {
        let mut lista = displays.to_vec();
        lista.sort_by_key(|m| (m.x, m.y, m.id));
        // Espelhados (mesmo retângulo) aparecem uma vez só.
        lista.dedup_by(|b, a| (a.x, a.y, a.width, a.height) == (b.x, b.y, b.width, b.height));
        self.monitores = lista;
        // Destacado que sumiu fecha a janela; o que deixou de ser liberado para o vídeo também.
        let monitores = &self.monitores;
        self.destacados
            .retain(|&d| match monitores.iter().find(|m| m.id == d) {
                Some(m) if m.shared => true,
                Some(_) => {
                    out.push(Msg::StopVideo(StopVideo { display_id: d }));
                    false
                }
                None => false,
            });
        let ativo_ok = self
            .ativo
            .is_some_and(|a| self.monitores.iter().any(|m| m.id == a && m.shared));
        if !ativo_ok {
            if let Some(antigo) = self.ativo.take()
                && self.monitores.iter().any(|m| m.id == antigo)
            {
                // Continua existindo, mas não está mais liberado.
                out.push(Msg::StopVideo(StopVideo { display_id: antigo }));
            }
            if let Some(novo) = self.padrao() {
                self.iniciar(novo, out);
            }
        }
    }

    fn padrao(&self) -> Option<u32> {
        let liberados = || {
            self.monitores
                .iter()
                .filter(|m| m.shared && !self.destacados.contains(&m.id))
        };
        liberados()
            .find(|m| m.primary)
            .or_else(|| liberados().next())
            .map(|m| m.id)
    }

    fn iniciar(&mut self, id: u32, out: &mut Vec<Msg>) {
        self.ativo = Some(id);
        out.push(Msg::StartVideo(StartVideo {
            display_id: id,
            wanted: None,
            priority: Priority::Focused as i32,
        }));
    }

    /// Troca para o monitor `id`. `false` se já é o ativo, não existe ou não foi liberado. Um
    /// destacado volta para a janela da sessão com o vídeo que já recebia.
    pub fn trocar(&mut self, id: u32, out: &mut Vec<Msg>) -> bool {
        if self.ativo == Some(id) || !self.monitores.iter().any(|m| m.id == id && m.shared) {
            return false;
        }
        if let Some(antigo) = self.ativo {
            out.push(Msg::StopVideo(StopVideo { display_id: antigo }));
        }
        if let Some(i) = self.destacados.iter().position(|&d| d == id) {
            self.destacados.remove(i);
            self.ativo = Some(id);
        } else {
            self.iniciar(id, out);
        }
        true
    }

    pub fn destacados(&self) -> &[u32] {
        &self.destacados
    }

    pub fn destacado(&self, id: u32) -> bool {
        self.destacados.contains(&id)
    }

    pub fn monitor(&self, id: u32) -> Option<&DisplayInfo> {
        self.monitores.iter().find(|m| m.id == id)
    }

    /// Abre o monitor `id` numa janela própria. Se é o que a sessão mostra, a sessão passa para
    /// outro liberado (ou fica sem monitor) e o vídeo continua sem reiniciar. `false` se não
    /// existe, não foi liberado ou já está destacado.
    pub fn destacar(&mut self, id: u32, out: &mut Vec<Msg>) -> bool {
        if self.destacado(id) || !self.monitores.iter().any(|m| m.id == id && m.shared) {
            return false;
        }
        self.destacados.push(id);
        if self.ativo == Some(id) {
            self.ativo = None;
            if let Some(novo) = self.padrao() {
                self.iniciar(novo, out);
            }
        } else {
            self.iniciar_destacado(id, out);
        }
        true
    }

    fn iniciar_destacado(&self, id: u32, out: &mut Vec<Msg>) {
        out.push(Msg::StartVideo(StartVideo {
            display_id: id,
            wanted: None,
            priority: Priority::Visible as i32,
        }));
    }

    /// A janela do destacado `id` fechou: volta a ser aba. Se a sessão estava sem monitor, ele
    /// vira o mostrado (o vídeo segue); senão o vídeo dele para.
    pub fn recolocar(&mut self, id: u32, out: &mut Vec<Msg>) -> bool {
        let Some(i) = self.destacados.iter().position(|&d| d == id) else {
            return false;
        };
        self.destacados.remove(i);
        if self.ativo.is_none() {
            self.ativo = Some(id);
        } else {
            out.push(Msg::StopVideo(StopVideo { display_id: id }));
        }
        true
    }

    /// Aplica um atalho. Anterior/próximo pulam os não liberados e dão a volta.
    pub fn aplicar(&mut self, a: Atalho, out: &mut Vec<Msg>) -> bool {
        let n = self.monitores.len();
        let alvo = match a {
            Atalho::Direto(numero) => {
                let i = usize::try_from(numero).ok().and_then(|v| v.checked_sub(1));
                i.and_then(|i| self.monitores.get(i)).map(|m| m.id)
            }
            Atalho::Anterior | Atalho::Proximo if n > 0 => {
                let atual = self
                    .ativo
                    .and_then(|a| self.monitores.iter().position(|m| m.id == a))
                    .unwrap_or(0);
                let passo = if a == Atalho::Proximo { 1 } else { n - 1 };
                (1..n)
                    .map(|k| &self.monitores[(atual + k * passo) % n])
                    .find(|m| m.shared)
                    .map(|m| m.id)
            }
            _ => None,
        };
        alvo.is_some_and(|id| self.trocar(id, out))
    }

    /// Com "seguir o mouse", a aba acompanha o cursor do host quando ele muda de monitor.
    pub fn on_cursor(&mut self, pos: &CursorPos, out: &mut Vec<Msg>) -> bool {
        // O cursor num destacado já está à vista na janela dele.
        self.seguir_mouse
            && pos.visible
            && !self.destacado(pos.display_id)
            && self.trocar(pos.display_id, out)
    }

    /// O host parou o vídeo (monitor removido, portal fechou): volta para o principal.
    pub fn on_video_stopped(&mut self, v: &VideoStopped, out: &mut Vec<Msg>) {
        if let Some(i) = self.destacados.iter().position(|&d| d == v.display_id) {
            self.destacados.remove(i);
            if let Some(m) = self.monitores.iter_mut().find(|m| m.id == v.display_id) {
                m.shared = false;
            }
            return;
        }
        if self.ativo != Some(v.display_id) {
            return;
        }
        self.ativo = None;
        if let Some(m) = self.monitores.iter_mut().find(|m| m.id == v.display_id) {
            m.shared = false;
        }
        if let Some(novo) = self.padrao() {
            self.iniciar(novo, out);
        }
    }

    /// Mouse local → `(display_id, x, y)` normalizados no monitor ativo. `area` é o tamanho do
    /// componente de vídeo e `local` a posição do mouse nele (mesma unidade); a imagem é
    /// desenhada inteira e centrada (`image-fit: contain`). Fora da imagem dá `None`, a não ser
    /// com `prender` (botão apertado arrastando), que leva para a borda.
    pub fn ponto(
        &self,
        area: (f32, f32),
        local: (f32, f32),
        prender: bool,
    ) -> Option<(u32, u16, u16)> {
        let m = self.monitor_ativo()?;
        ponto_contido(area, (m.width, m.height), local, prender).map(|(x, y)| (m.id, x, y))
    }

    /// [`Self::ponto`] na janela do destacado `id`.
    pub fn ponto_destacado(
        &self,
        id: u32,
        area: (f32, f32),
        local: (f32, f32),
        prender: bool,
    ) -> Option<(u32, u16, u16)> {
        if !self.destacado(id) {
            return None;
        }
        let m = self.monitor(id)?;
        ponto_contido(area, (m.width, m.height), local, prender).map(|(x, y)| (m.id, x, y))
    }
}

fn ponto_contido(
    area: (f32, f32),
    imagem: (u32, u32),
    local: (f32, f32),
    prender: bool,
) -> Option<(u16, u16)> {
    let (iw, ih) = (imagem.0 as f32, imagem.1 as f32);
    if area.0 <= 0.0 || area.1 <= 0.0 || iw <= 0.0 || ih <= 0.0 {
        return None;
    }
    let escala = (area.0 / iw).min(area.1 / ih);
    let (w, h) = (iw * escala, ih * escala);
    let (ox, oy) = ((area.0 - w) / 2.0, (area.1 - h) / 2.0);
    let (fx, fy) = ((local.0 - ox) / w, (local.1 - oy) / h);
    let dentro = (0.0..=1.0).contains(&fx) && (0.0..=1.0).contains(&fy);
    if !dentro && !prender {
        return None;
    }
    let norm = |f: f32| (f.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16;
    Some((norm(fx), norm(fy)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(id: u32, x: i32, primary: bool, shared: bool) -> DisplayInfo {
        DisplayInfo {
            id,
            name: format!("M{id}"),
            x,
            y: 0,
            width: 1920,
            height: 1080,
            primary,
            shared,
            ..DisplayInfo::default()
        }
    }

    fn start(id: u32) -> Msg {
        Msg::StartVideo(StartVideo {
            display_id: id,
            wanted: None,
            priority: Priority::Focused as i32,
        })
    }

    fn stop(id: u32) -> Msg {
        Msg::StopVideo(StopVideo { display_id: id })
    }

    /// Host com 3 monitores: o principal no meio, o da direita não liberado (Wayland).
    fn tres() -> (UmPorVez, Vec<Msg>) {
        let mut u = UmPorVez::new();
        let mut out = Vec::new();
        u.atualizar(
            &[
                mon(7, 1920, true, true),
                mon(3, 3840, false, false),
                mon(5, -1920 + 1920, false, true),
            ],
            &mut out,
        );
        (u, out)
    }

    #[test]
    fn abre_o_principal_e_numera_da_esquerda_para_a_direita() {
        let (u, out) = tres();
        assert_eq!(out, [start(7)]);
        let abas = u.abas();
        assert!(u.mostrar_abas());
        assert_eq!(
            abas.iter()
                .map(|a| (a.numero, a.display_id, a.ativa, a.liberado))
                .collect::<Vec<_>>(),
            [
                (1, 5, false, true),
                (2, 7, true, true),
                (3, 3, false, false)
            ]
        );
        assert_eq!(abas[1].rotulo, "M7 · 1920×1080");
        assert!(abas[1].principal);
    }

    #[test]
    fn trocar_e_stop_mais_start() {
        let (mut u, _) = tres();
        let mut out = Vec::new();
        assert!(u.trocar(5, &mut out));
        assert_eq!(out, [stop(7), start(5)]);
        out.clear();
        assert!(!u.trocar(5, &mut out));
        assert!(!u.trocar(3, &mut out), "não liberado");
        assert!(!u.trocar(99, &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn atalhos() {
        let ca = Mods::from_bits(Mods::CTRL.bits() | Mods::ALT.bits());
        assert_eq!(atalho(0x1E, ca), Some(Atalho::Direto(1)));
        assert_eq!(atalho(0x26, ca), Some(Atalho::Direto(9)));
        assert_eq!(atalho(0x4F, ca), Some(Atalho::Proximo));
        assert_eq!(atalho(0x50, ca), Some(Atalho::Anterior));
        assert_eq!(atalho(0x1E, Mods::CTRL), None);
        let cas = Mods::from_bits(ca.bits() | Mods::SHIFT.bits());
        assert_eq!(atalho(0x1E, cas), None);
        assert_eq!(atalho(0x27, ca), None, "Ctrl+Alt+0 não é atalho");

        let (mut u, _) = tres();
        let mut out = Vec::new();
        // Próximo pula o não liberado e dá a volta.
        assert!(u.aplicar(Atalho::Proximo, &mut out));
        assert_eq!(u.ativo(), Some(5));
        assert!(u.aplicar(Atalho::Anterior, &mut out));
        assert_eq!(u.ativo(), Some(7));
        assert!(u.aplicar(Atalho::Direto(1), &mut out));
        assert_eq!(u.ativo(), Some(5));
        assert!(!u.aplicar(Atalho::Direto(3), &mut out));
        assert!(!u.aplicar(Atalho::Direto(9), &mut out));
        assert_eq!(u.ativo(), Some(5));
    }

    #[test]
    fn seguir_o_mouse() {
        let (mut u, _) = tres();
        let mut out = Vec::new();
        let pos = CursorPos {
            display_id: 5,
            x: 10,
            y: 10,
            visible: true,
        };
        assert!(!u.on_cursor(&pos, &mut out));
        u.seguir_mouse = true;
        assert!(u.on_cursor(&pos, &mut out));
        assert_eq!(out, [stop(7), start(5)]);
    }

    #[test]
    fn monitor_removido_ou_parado_volta_ao_principal() {
        let (mut u, _) = tres();
        let mut out = Vec::new();
        u.trocar(5, &mut out);
        out.clear();
        u.atualizar(
            &[mon(7, 1920, true, true), mon(3, 3840, false, false)],
            &mut out,
        );
        assert_eq!(out, [start(7)]);
        assert!(u.mostrar_abas());

        out.clear();
        u.on_video_stopped(
            &VideoStopped {
                display_id: 7,
                reason: 2,
            },
            &mut out,
        );
        assert!(out.is_empty(), "nenhum outro liberado");
        assert_eq!(u.ativo(), None);

        // Host com um monitor só: sem abas.
        let mut u = UmPorVez::new();
        u.atualizar(&[mon(1, 0, true, true)], &mut out);
        assert!(!u.mostrar_abas());
    }

    fn visivel(id: u32) -> Msg {
        Msg::StartVideo(StartVideo {
            display_id: id,
            wanted: None,
            priority: Priority::Visible as i32,
        })
    }

    #[test]
    fn destacar_outro_monitor_abre_mais_um_stream() {
        let (mut u, _) = tres();
        let mut out = Vec::new();
        assert!(u.destacar(5, &mut out));
        assert_eq!(out, [visivel(5)]);
        assert_eq!(u.ativo(), Some(7));
        assert!(u.abas()[0].destacada);
        assert!(!u.destacar(5, &mut out), "já destacado");
        assert!(!u.destacar(3, &mut out), "não liberado");
        // Fechar a janela para o stream dele.
        out.clear();
        assert!(u.recolocar(5, &mut out));
        assert_eq!(out, [stop(5)]);
        assert!(!u.recolocar(5, &mut out));
    }

    #[test]
    fn destacar_o_monitor_mostrado_nao_reinicia_o_video() {
        let (mut u, _) = tres();
        let mut out = Vec::new();
        assert!(u.destacar(7, &mut out));
        // A sessão passa para o outro liberado; o 7 segue com o mesmo stream.
        assert_eq!(out, [start(5)]);
        assert_eq!(u.ativo(), Some(5));
        // Os dois destacados: a sessão fica sem monitor.
        out.clear();
        assert!(u.destacar(5, &mut out));
        assert!(out.is_empty());
        assert_eq!(u.ativo(), None);
        // Fechar a janela do 5 o devolve à sessão, sem mexer no vídeo.
        assert!(u.recolocar(5, &mut out));
        assert!(out.is_empty());
        assert_eq!(u.ativo(), Some(5));
        // Clicar na aba do 7 (destacado) o traz de volta: só para o 5.
        assert!(u.trocar(7, &mut out));
        assert_eq!(out, [stop(5)]);
        assert_eq!(u.ativo(), Some(7));
        assert!(u.destacados().is_empty());
    }

    #[test]
    fn destacado_que_some_fecha() {
        let (mut u, _) = tres();
        let mut out = Vec::new();
        u.destacar(5, &mut out);
        out.clear();
        u.atualizar(
            &[mon(7, 1920, true, true), mon(3, 3840, false, false)],
            &mut out,
        );
        assert!(out.is_empty());
        assert!(u.destacados().is_empty());

        let (mut u, _) = tres();
        u.destacar(5, &mut out);
        out.clear();
        u.on_video_stopped(
            &VideoStopped {
                display_id: 5,
                reason: 2,
            },
            &mut out,
        );
        assert!(out.is_empty());
        assert!(u.destacados().is_empty());
        assert_eq!(u.ativo(), Some(7));
        // Seguir o mouse não puxa um destacado para a sessão.
        let (mut u, _) = tres();
        u.destacar(5, &mut out);
        u.seguir_mouse = true;
        out.clear();
        let pos = CursorPos {
            display_id: 5,
            x: 1,
            y: 1,
            visible: true,
        };
        assert!(!u.on_cursor(&pos, &mut out));
        assert_eq!(
            u.ponto_destacado(5, (1920.0, 1080.0), (960.0, 540.0), false),
            Some((5, 32768, 32768))
        );
        assert_eq!(
            u.ponto_destacado(7, (1920.0, 1080.0), (960.0, 540.0), false),
            None
        );
    }

    #[test]
    fn espelhados_aparecem_uma_vez() {
        let mut u = UmPorVez::new();
        let mut out = Vec::new();
        u.atualizar(&[mon(1, 0, true, true), mon(2, 0, false, true)], &mut out);
        assert_eq!(u.abas().len(), 1);
    }

    #[test]
    fn mouse_no_monitor_com_barras() {
        let (u, _) = tres();
        // Área 2000×900 para imagem 16:9: escala 900/1080, largura 1600, barras de 200 px.
        let area = (2000.0, 900.0);
        assert_eq!(
            u.ponto(area, (1000.0, 450.0), false),
            Some((7, 32768, 32768))
        );
        assert_eq!(u.ponto(area, (200.0, 0.0), false), Some((7, 0, 0)));
        assert_eq!(
            u.ponto(area, (1800.0, 900.0), false),
            Some((7, 65535, 65535))
        );
        assert_eq!(u.ponto(area, (10.0, 450.0), false), None);
        assert_eq!(u.ponto(area, (10.0, 450.0), true), Some((7, 0, 32768)));
        assert_eq!(UmPorVez::new().ponto(area, (1.0, 1.0), false), None);
    }
}
