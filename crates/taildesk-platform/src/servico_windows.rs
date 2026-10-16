//! Serviço do Windows "TailDesk": roda como SYSTEM desde o boot e mantém o app aberto (na
//! bandeja, `--oculto`) na sessão de quem está no console, para o acesso com senha funcionar sem
//! ninguém abrir o TailDesk.
//!
//! O serviço em si não captura nem injeta nada (a sessão 0 não tem tela): ele só lança o app com
//! o token do usuário logado (`WTSQueryUserToken` + `CreateProcessAsUserW`), lança de novo quando
//! alguém entra (`WTS_SESSION_LOGON`) e se o app cair. Se a pessoa sai pelo menu da bandeja
//! (código 0), espera a próxima entrada.
//!
//! Instalar e remover pedem administrador: o app se chama de novo com `runas` e
//! `--instalar-servico` / `--remover-servico` ([`elevar`]).

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_EXISTS,
    ERROR_SERVICE_NOT_ACTIVE, HANDLE, NO_ERROR, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::DELETE;
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, RegisterServiceCtrlHandlerExW, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_CREATE_SERVICE, SERVICE_ACCEPT_SESSIONCHANGE, SERVICE_ACCEPT_STOP,
    SERVICE_ALL_ACCESS, SERVICE_AUTO_START, SERVICE_CONFIG_DESCRIPTION,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SESSIONCHANGE, SERVICE_CONTROL_STOP,
    SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
    SERVICE_START, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOP,
    SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
    SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW,
};
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateEventW, CreateProcessAsUserW, GetExitCodeProcess, INFINITE,
    PROCESS_INFORMATION, STARTUPINFOW, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, WTS_SESSION_LOGON};

/// Nome do serviço no Gerenciador de Serviços.
pub const NOME: &str = "TailDesk";
const DESCRICAO: &str = "Mantém o TailDesk aberto para receber conexões da sua tailnet (acesso \
                         remoto com senha). Remova em Configurações do TailDesk.";
/// Sem sessão de console (troca de usuário em andamento).
const SEM_SESSAO: u32 = 0xFFFF_FFFF;
/// Espera antes de lançar de novo quando o app caiu ou ainda não há usuário logado.
const DE_NOVO: Duration = Duration::from_secs(5);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Fecha um `SC_HANDLE` ao sair do escopo.
struct Sc(SC_HANDLE);

impl Drop for Sc {
    fn drop(&mut self) {
        // SAFETY: handle aberto por `OpenSCManagerW`/`OpenServiceW`/`CreateServiceW`.
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn gerenciador(acesso: u32) -> io::Result<Sc> {
    // SAFETY: nulos = este computador, banco padrão.
    let h = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), acesso) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(Sc(h))
}

fn abrir_servico(scm: &Sc, acesso: u32) -> io::Result<Sc> {
    let nome = wide(NOME);
    // SAFETY: gerenciador aberto, nome terminado em zero.
    let h = unsafe { OpenServiceW(scm.0, nome.as_ptr(), acesso) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(Sc(h))
}

fn iniciar(s: &Sc) -> io::Result<()> {
    // SAFETY: serviço aberto com SERVICE_START, sem argumentos.
    if unsafe { StartServiceW(s.0, 0, std::ptr::null()) } == 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(ERROR_SERVICE_ALREADY_RUNNING as i32) {
            return Err(e);
        }
    }
    Ok(())
}

/// Instala (ou só inicia, se já existe) o serviço que lança `exe --oculto`. Precisa de
/// administrador.
pub fn instalar(exe: &Path) -> io::Result<()> {
    let scm = gerenciador(SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE)?;
    match abrir_servico(&scm, SERVICE_START) {
        Ok(s) => return iniciar(&s),
        Err(e) if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) => {}
        Err(e) => return Err(e),
    }
    let nome = wide(NOME);
    let comando = wide(&format!("\"{}\" --servico", exe.display()));
    // SAFETY: strings terminadas em zero vivas durante a chamada; nulos = sem grupo, sem
    // dependências, conta LocalSystem.
    let h = unsafe {
        CreateServiceW(
            scm.0,
            nome.as_ptr(),
            nome.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            comando.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if h.is_null() {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_SERVICE_EXISTS as i32) {
            return iniciar(&abrir_servico(&scm, SERVICE_START)?);
        }
        return Err(e);
    }
    let s = Sc(h);
    let mut texto = wide(DESCRICAO);
    let d = SERVICE_DESCRIPTIONW {
        lpDescription: texto.as_mut_ptr(),
    };
    // SAFETY: serviço recém-criado; a descrição vive durante a chamada. Sem descrição o
    // serviço funciona igual.
    unsafe { ChangeServiceConfig2W(s.0, SERVICE_CONFIG_DESCRIPTION, (&raw const d).cast()) };
    iniciar(&s)
}

/// Para e remove o serviço (o app aberto na sessão continua). Precisa de administrador.
pub fn remover() -> io::Result<()> {
    let scm = gerenciador(SC_MANAGER_CONNECT)?;
    let s = match abrir_servico(&scm, SERVICE_STOP | DELETE) {
        Ok(s) => s,
        Err(e) if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) => return Ok(()),
        Err(e) => return Err(e),
    };
    // SAFETY: zero é um SERVICE_STATUS válido; serviço aberto com SERVICE_STOP.
    unsafe {
        let mut st: SERVICE_STATUS = std::mem::zeroed();
        if ControlService(s.0, SERVICE_CONTROL_STOP, &mut st) == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(ERROR_SERVICE_NOT_ACTIVE as i32) {
                eprintln!("taildesk: não foi possível parar o serviço: {e}");
            }
        }
        if DeleteService(s.0) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// O serviço existe.
pub fn instalado() -> bool {
    gerenciador(SC_MANAGER_CONNECT)
        .is_ok_and(|scm| abrir_servico(&scm, SERVICE_QUERY_STATUS).is_ok())
}

/// Roda `exe argumento` como administrador (o Windows pergunta) e espera. `Err` se a pessoa
/// recusou ou o comando falhou.
pub fn elevar(exe: &Path, argumento: &str) -> io::Result<()> {
    let verbo = wide("runas");
    let arquivo = wide(&exe.to_string_lossy());
    let parametros = wide(argumento);
    // SAFETY: zero é válido para os campos não usados; strings vivas até o fim da espera.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verbo.as_ptr();
        info.lpFile = arquivo.as_ptr();
        info.lpParameters = parametros.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.hProcess.is_null() {
            return Ok(());
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut codigo = 1;
        GetExitCodeProcess(info.hProcess, &mut codigo);
        CloseHandle(info.hProcess);
        if codigo == 0 {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "o comando terminou com código {codigo}"
            )))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// O serviço rodando

/// Eventos do serviço: parar e alguém entrou. Postos antes do dispatcher.
static PARAR: AtomicIsize = AtomicIsize::new(0);
static ENTROU: AtomicIsize = AtomicIsize::new(0);
static STATUS: AtomicIsize = AtomicIsize::new(0);

fn informar(estado: u32) {
    let st = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: estado,
        dwControlsAccepted: if estado == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SESSIONCHANGE
        } else {
            0
        },
        dwWin32ExitCode: NO_ERROR,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: if estado == SERVICE_RUNNING { 0 } else { 3000 },
    };
    // SAFETY: handle de `RegisterServiceCtrlHandlerExW`; a estrutura vive durante a chamada.
    unsafe { SetServiceStatus(STATUS.load(Ordering::Acquire) as SERVICE_STATUS_HANDLE, &st) };
}

unsafe extern "system" fn controle(
    codigo: u32,
    tipo: u32,
    _: *mut core::ffi::c_void,
    _: *mut core::ffi::c_void,
) -> u32 {
    match codigo {
        SERVICE_CONTROL_STOP => {
            informar(SERVICE_STOP_PENDING);
            // SAFETY: evento criado em `rodar` antes do dispatcher, nunca fechado.
            unsafe { SetEvent(PARAR.load(Ordering::Acquire) as HANDLE) };
            NO_ERROR
        }
        SERVICE_CONTROL_SESSIONCHANGE => {
            if tipo == WTS_SESSION_LOGON {
                // SAFETY: idem.
                unsafe { SetEvent(ENTROU.load(Ordering::Acquire) as HANDLE) };
            }
            NO_ERROR
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR,
        _ => windows_sys::Win32::Foundation::ERROR_CALL_NOT_IMPLEMENTED,
    }
}

/// O app aberto na sessão do usuário.
struct Filho {
    processo: HANDLE,
}

impl Drop for Filho {
    fn drop(&mut self) {
        // SAFETY: handle de `CreateProcessAsUserW`, fechado só aqui.
        unsafe { CloseHandle(self.processo) };
    }
}

/// Lança `exe --oculto` na sessão `sessao`, como o usuário dela.
fn lancar(exe: &Path, sessao: u32) -> io::Result<Filho> {
    let mut linha = wide(&format!("\"{}\" --oculto", exe.display()));
    let mut mesa = wide("winsta0\\default");
    // SAFETY: token e bloco de ambiente são liberados no fim; estruturas zeradas são válidas;
    // a linha de comando é mutável como a API pede.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if WTSQueryUserToken(sessao, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut ambiente: *mut core::ffi::c_void = std::ptr::null_mut();
        if CreateEnvironmentBlock(&mut ambiente, token, 0) == 0 {
            ambiente = std::ptr::null_mut();
        }
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = mesa.as_mut_ptr();
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessAsUserW(
            token,
            std::ptr::null(),
            linha.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_UNICODE_ENVIRONMENT,
            ambiente,
            std::ptr::null(),
            &si,
            &mut pi,
        );
        let erro = io::Error::last_os_error();
        if !ambiente.is_null() {
            DestroyEnvironmentBlock(ambiente);
        }
        CloseHandle(token);
        if ok == 0 {
            return Err(erro);
        }
        CloseHandle(pi.hThread);
        Ok(Filho {
            processo: pi.hProcess,
        })
    }
}

unsafe extern "system" fn principal(_: u32, _: *mut windows_sys::core::PWSTR) {
    let nome = wide(NOME);
    // SAFETY: nome terminado em zero; o handler é uma função estática.
    let h =
        unsafe { RegisterServiceCtrlHandlerExW(nome.as_ptr(), Some(controle), std::ptr::null()) };
    if h.is_null() {
        return;
    }
    STATUS.store(h as isize, Ordering::Release);
    informar(SERVICE_START_PENDING);
    let Ok(exe) = std::env::current_exe() else {
        informar(SERVICE_STOPPED);
        return;
    };
    informar(SERVICE_RUNNING);

    let parar = PARAR.load(Ordering::Acquire) as HANDLE;
    let entrou = ENTROU.load(Ordering::Acquire) as HANDLE;
    let mut filho: Option<Filho> = None;
    // A pessoa saiu pela bandeja: só na próxima entrada.
    let mut pausado = false;
    let mut proxima = Instant::now();
    loop {
        if filho.is_none() && !pausado && Instant::now() >= proxima {
            // SAFETY: sem pré-condições.
            let sessao = unsafe { WTSGetActiveConsoleSessionId() };
            if sessao != SEM_SESSAO {
                match lancar(&exe, sessao) {
                    Ok(f) => filho = Some(f),
                    // Ninguém logado ainda (tela de entrada): tenta de novo.
                    Err(_) => proxima = Instant::now() + DE_NOVO,
                }
            }
        }
        let mut esperar = vec![parar, entrou];
        if let Some(f) = &filho {
            esperar.push(f.processo);
        }
        // SAFETY: handles válidos enquanto `filho` vive; falso = qualquer um acorda.
        let r = unsafe {
            WaitForMultipleObjects(
                esperar.len() as u32,
                esperar.as_ptr(),
                0,
                DE_NOVO.as_millis() as u32,
            )
        };
        match r.wrapping_sub(WAIT_OBJECT_0) {
            0 => break,
            1 => {
                pausado = false;
                proxima = Instant::now();
            }
            2 => {
                let mut codigo = 1;
                if let Some(f) = filho.take() {
                    // SAFETY: processo terminado, handle ainda aberto.
                    unsafe { GetExitCodeProcess(f.processo, &mut codigo) };
                }
                pausado = codigo == 0;
                proxima = Instant::now() + DE_NOVO;
            }
            _ => {}
        }
    }
    informar(SERVICE_STOPPED);
}

/// `taildesk --servico`: entra no dispatcher do Windows e só volta quando o serviço para.
/// Fora do Gerenciador de Serviços falha logo.
pub fn rodar() -> io::Result<()> {
    // SAFETY: eventos sem nome que vivem até o processo acabar: PARAR fica sinalizado (reset
    // manual), ENTROU volta sozinho depois de acordar o laço.
    unsafe {
        PARAR.store(
            CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) as isize,
            Ordering::Release,
        );
        ENTROU.store(
            CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) as isize,
            Ordering::Release,
        );
    }
    let mut nome = wide(NOME);
    let tabela = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: nome.as_mut_ptr(),
            lpServiceProc: Some(principal),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    // SAFETY: tabela terminada pela entrada nula; vive até o dispatcher voltar.
    if unsafe { StartServiceCtrlDispatcherW(tabela.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
