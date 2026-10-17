//! Área de trabalho de entrada do Windows (`Default`, `Winlogon`, `Screen-saver`).
//!
//! O Windows troca a área de trabalho que recebe teclado e mouse na tela de entrada, no
//! bloqueio (Win+L), no Ctrl+Alt+Del e no pedido do UAC (área segura). `SendInput` e a Desktop
//! Duplication só funcionam na área de trabalho da própria thread, então quem injeta ou captura
//! chama [`seguir_entrada`] antes: a thread passa para a área de entrada atual.
//!
//! Só o agente do serviço (SYSTEM na sessão do console, `servico_windows`) consegue abrir a
//! `Winlogon`; o app comum, rodando como o usuário, recebe acesso negado ali e continua na
//! `Default` (a tela segura fica preta para quem está do outro lado, como antes).

use std::cell::Cell;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_ACCESS_FLAGS, GetThreadDesktop, GetUserObjectInformationW, HDESK,
    OpenInputDesktop, SetThreadDesktop, UOI_NAME,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

/// `GENERIC_ALL`: ler, escrever, criar janelas, ganchos, `SendInput` e captura.
const ACESSO: DESKTOP_ACCESS_FLAGS = 0x1000_0000;

/// De quanto em quanto tempo [`seguir_entrada_de_vez_em_quando`] confere a área de entrada.
const INTERVALO: Duration = Duration::from_millis(200);

thread_local! {
    /// Área aberta por esta thread (fechada quando a thread troca de novo).
    static ABERTA: Cell<HDESK> = const { Cell::new(std::ptr::null_mut()) };
    static ULTIMA: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// [`seguir_entrada`] no máximo a cada 200 ms por thread: para o caminho de cada tecla e de
/// cada movimento do mouse, onde abrir a área de entrada toda vez custaria à toa.
pub fn seguir_entrada_de_vez_em_quando() {
    let agora = Instant::now();
    if ULTIMA.with(|u| u.get().is_some_and(|t| agora.duration_since(t) < INTERVALO)) {
        return;
    }
    ULTIMA.with(|u| u.set(Some(agora)));
    seguir_entrada();
}

/// Nome de uma área de trabalho (`Default`, `Winlogon`...).
fn nome(h: HDESK) -> Option<String> {
    let mut buf = [0u16; 64];
    let mut usado = 0u32;
    // SAFETY: buffer local com o tamanho em bytes informado; `usado` recebe o tamanho.
    let ok = unsafe {
        GetUserObjectInformationW(
            h,
            UOI_NAME,
            buf.as_mut_ptr().cast(),
            std::mem::size_of_val(&buf) as u32,
            &mut usado,
        )
    };
    if ok == 0 {
        return None;
    }
    let fim = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..fim]))
}

/// Nome da área de trabalho que recebe a entrada agora, se este processo pode abri-la.
pub fn nome_da_entrada() -> Option<String> {
    // SAFETY: sem pré-condições; o handle é fechado logo depois.
    unsafe {
        let h = OpenInputDesktop(0, 0, ACESSO);
        if h.is_null() {
            return None;
        }
        let n = nome(h);
        CloseDesktop(h);
        n
    }
}

/// Põe a thread atual na área de trabalho de entrada. `true` se já estava ou se trocou;
/// `false` se não dá para abrir (processo sem acesso à `Winlogon`) ou a thread tem janelas.
pub fn seguir_entrada() -> bool {
    // SAFETY: handles do próprio processo; `SetThreadDesktop` falha (sem efeito) se a thread
    // tem janelas ou ganchos; o handle antigo só é fechado depois que a thread saiu dele.
    unsafe {
        let entrada = OpenInputDesktop(0, 0, ACESSO);
        if entrada.is_null() {
            return false;
        }
        let atual = GetThreadDesktop(GetCurrentThreadId());
        if !atual.is_null() && nome(atual).is_some_and(|n| Some(n) == nome(entrada)) {
            CloseDesktop(entrada);
            return true;
        }
        if SetThreadDesktop(entrada) == 0 {
            CloseDesktop(entrada);
            return false;
        }
        let anterior = ABERTA.with(|a| a.replace(entrada));
        if !anterior.is_null() {
            CloseDesktop(anterior);
        }
        true
    }
}

/// Ctrl+Alt+Del de verdade (a sequência de atenção segura). `asuser = false` exige SYSTEM
/// (o agente do serviço); de um processo comum, `true`, que só funciona se a política
/// `SoftwareSASGeneration` permitir. O controlador não consegue mandar isso como teclas: o
/// próprio Windows dele intercepta Ctrl+Alt+Del.
pub fn ctrl_alt_del(como_sistema: bool) {
    use windows_sys::Win32::Security::Authentication::Identity::SendSAS;
    // SAFETY: sem pré-condições; sem privilégio o Windows ignora a chamada.
    unsafe { SendSAS(windows_sys::core::BOOL::from(!como_sistema)) };
}
