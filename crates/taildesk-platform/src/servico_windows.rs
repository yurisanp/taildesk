//! Serviço do Windows "TailDesk": roda como SYSTEM desde o boot e mantém o app aberto (na
//! bandeja, `--oculto`) na sessão de quem está no console, para o acesso com senha funcionar sem
//! ninguém abrir o TailDesk.
//!
//! O serviço em si não captura nem injeta nada (a sessão 0 não tem tela): ele só lança o app com
//! o token do usuário logado (`WTSQueryUserToken` + `CreateProcessAsUserW`), lança de novo quando
//! alguém entra (`WTS_SESSION_LOGON`) e se o app cair. Se a pessoa sai pelo menu da bandeja
//! (código 0), espera a próxima entrada.
//!
//! Sem ninguém logado no console (tela de entrada, depois de reiniciar), lança o **agente**:
//! `taildesk --agente <perfil>` como SYSTEM na sessão do console (o token do próprio serviço com
//! outra `TokenSessionId`). Só SYSTEM abre a área `Winlogon`, então o agente captura e controla a
//! tela de entrada e atende Ctrl+Alt+Del. Ele usa a identidade e a senha de acesso do último
//! usuário que o serviço viu (`<perfil>`, guardado no registro do serviço, que só administradores
//! mudam). Quando alguém entra, o agente é encerrado e o app do usuário volta a ser o host.
//!
//! Instalar e remover pedem administrador: o app se chama de novo com `runas` e
//! `--instalar-servico` / `--remover-servico` ([`elevar`]).

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use std::path::PathBuf;

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
    PROCESS_INFORMATION, STARTUPINFOW, SetEvent, TerminateProcess, WaitForMultipleObjects,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SW_HIDE, WTS_CONSOLE_CONNECT, WTS_SESSION_LOGOFF, WTS_SESSION_LOGON,
};

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
            // Entrou, saiu ou o console trocou de sessão: o laço decide de novo.
            if matches!(
                tipo,
                WTS_SESSION_LOGON | WTS_SESSION_LOGOFF | WTS_CONSOLE_CONNECT
            ) {
                // SAFETY: idem.
                unsafe { SetEvent(ENTROU.load(Ordering::Acquire) as HANDLE) };
            }
            NO_ERROR
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR,
        _ => windows_sys::Win32::Foundation::ERROR_CALL_NOT_IMPLEMENTED,
    }
}

/// O que o serviço mantém aberto na sessão do console.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tipo {
    /// O app, como o usuário logado.
    App,
    /// O agente da tela de entrada, como SYSTEM.
    Agente,
}

/// Processo lançado pelo serviço.
struct Filho {
    processo: HANDLE,
    tipo: Tipo,
}

impl Drop for Filho {
    fn drop(&mut self) {
        // SAFETY: handle de `CreateProcessAsUserW`, fechado só aqui.
        unsafe { CloseHandle(self.processo) };
    }
}

/// Próximo passo do laço do serviço.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Acao {
    Nada,
    Lancar(Tipo),
    /// Alguém entrou com o agente aberto: encerra o agente (o app vem no passo seguinte).
    EncerrarAgente,
}

/// Decide o passo: `aberto` é o que está rodando, `logado` se há usuário no console,
/// `pausado` se a pessoa saiu pela bandeja, `pode` se já passou a espera desde a última
/// tentativa e `tem_perfil` se o serviço já viu um usuário (de onde o agente tira identidade e
/// senha).
fn decidir(
    aberto: Option<Tipo>,
    logado: bool,
    pausado: bool,
    pode: bool,
    tem_perfil: bool,
) -> Acao {
    match aberto {
        Some(Tipo::Agente) if logado => Acao::EncerrarAgente,
        Some(_) => Acao::Nada,
        None if !pode => Acao::Nada,
        None if logado && !pausado => Acao::Lancar(Tipo::App),
        None if !logado && tem_perfil => Acao::Lancar(Tipo::Agente),
        None => Acao::Nada,
    }
}

/// Token do usuário logado na sessão, se houver.
fn token_do_usuario(sessao: u32) -> Option<HANDLE> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: saída num HANDLE local; quem recebe fecha.
    (unsafe { WTSQueryUserToken(sessao, &mut token) } != 0).then_some(token)
}

/// Pasta do perfil do dono de `token` (`C:\Users\fulano`).
fn perfil_do_token(token: HANDLE) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Shell::GetUserProfileDirectoryW;
    let mut buf = [0u16; 512];
    let mut n = buf.len() as u32;
    // SAFETY: buffer local com o tamanho em caracteres informado.
    if unsafe { GetUserProfileDirectoryW(token, buf.as_mut_ptr(), &mut n) } == 0 {
        return None;
    }
    let fim = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..fim])))
}

/// Chave de parâmetros do serviço (`HKLM\...\Services\TailDesk\Parameters`): só SYSTEM e
/// administradores escrevem nela.
fn chave_parametros() -> Vec<u16> {
    wide(&format!(
        r"SYSTEM\CurrentControlSet\Services\{NOME}\Parameters"
    ))
}

const VALOR_PERFIL: &str = "UltimoPerfil";

/// Lembra o perfil do último usuário (para o agente depois de reiniciar).
fn gravar_perfil(perfil: &Path) {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, REG_SZ, RegSetKeyValueW};
    let chave = chave_parametros();
    let nome = wide(VALOR_PERFIL);
    let dado = wide(&perfil.to_string_lossy());
    // SAFETY: strings terminadas em zero vivas durante a chamada; o tamanho é em bytes e
    // inclui o zero final, como REG_SZ pede.
    unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            chave.as_ptr(),
            nome.as_ptr(),
            REG_SZ,
            dado.as_ptr().cast(),
            (dado.len() * 2) as u32,
        )
    };
}

/// O perfil gravado por [`gravar_perfil`], se a pasta ainda existe.
fn ler_perfil() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let chave = chave_parametros();
    let nome = wide(VALOR_PERFIL);
    let mut buf = [0u16; 512];
    let mut bytes = std::mem::size_of_val(&buf) as u32;
    // SAFETY: buffer local com o tamanho em bytes informado.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            chave.as_ptr(),
            nome.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if r != NO_ERROR {
        return None;
    }
    let fim = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let p = PathBuf::from(String::from_utf16_lossy(&buf[..fim]));
    p.is_dir().then_some(p)
}

/// Cria o processo `linha` com `token` na área `winsta0\default`.
///
/// # Safety
/// `token` é um token primário válido; continua sendo de quem chamou.
unsafe fn criar_processo(token: HANDLE, linha: &str, tipo: Tipo) -> io::Result<Filho> {
    let mut linha = wide(linha);
    let mut mesa = wide("winsta0\\default");
    // SAFETY: bloco de ambiente liberado no fim; estruturas zeradas são válidas; a linha de
    // comando é mutável como a API pede; `token` válido pela pré-condição.
    unsafe {
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
        if ok == 0 {
            return Err(erro);
        }
        CloseHandle(pi.hThread);
        Ok(Filho {
            processo: pi.hProcess,
            tipo,
        })
    }
}

/// Lança `exe --oculto` como o dono de `token` e devolve também o perfil dele. Fecha o token.
fn lancar_app(exe: &Path, token: HANDLE) -> io::Result<(Filho, Option<PathBuf>)> {
    let perfil = perfil_do_token(token);
    // SAFETY: token de `WTSQueryUserToken`, válido até o `CloseHandle` abaixo.
    let r = unsafe { criar_processo(token, &format!("\"{}\" --oculto", exe.display()), Tipo::App) };
    // SAFETY: token nosso, fechado uma vez.
    unsafe { CloseHandle(token) };
    r.map(|f| (f, perfil))
}

/// Lança o agente da tela de entrada como SYSTEM na sessão `sessao`.
fn lancar_agente(exe: &Path, sessao: u32, perfil: &Path) -> io::Result<Filho> {
    use windows_sys::Win32::Security::{
        DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TOKEN_ALL_ACCESS,
        TokenPrimary, TokenSessionId,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let linha = format!("\"{}\" --agente \"{}\"", exe.display(), perfil.display());
    // SAFETY: tokens locais fechados no fim; o primário duplicado recebe a sessão do console
    // antes de criar o processo (o serviço roda como SYSTEM, que tem SeTcbPrivilege).
    unsafe {
        let mut proprio: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut proprio) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut token: HANDLE = std::ptr::null_mut();
        let ok = DuplicateTokenEx(
            proprio,
            TOKEN_ALL_ACCESS,
            std::ptr::null(),
            SecurityImpersonation,
            TokenPrimary,
            &mut token,
        );
        CloseHandle(proprio);
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let r = if SetTokenInformation(
            token,
            TokenSessionId,
            (&raw const sessao).cast(),
            std::mem::size_of::<u32>() as u32,
        ) == 0
        {
            Err(io::Error::last_os_error())
        } else {
            criar_processo(token, &linha, Tipo::Agente)
        };
        CloseHandle(token);
        r
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
    let mut perfil = ler_perfil();
    loop {
        // SAFETY: sem pré-condições.
        let sessao = unsafe { WTSGetActiveConsoleSessionId() };
        let token = (sessao != SEM_SESSAO)
            .then(|| token_do_usuario(sessao))
            .flatten();
        let acao = decidir(
            filho.as_ref().map(|f| f.tipo),
            token.is_some(),
            pausado,
            sessao != SEM_SESSAO && Instant::now() >= proxima,
            perfil.is_some(),
        );
        match acao {
            Acao::Lancar(Tipo::App) => {
                let t = token.expect("decidir só lança o app com usuário logado");
                match lancar_app(&exe, t) {
                    Ok((f, p)) => {
                        if let Some(p) = p.filter(|p| perfil.as_deref() != Some(p.as_path())) {
                            gravar_perfil(&p);
                            perfil = Some(p);
                        }
                        filho = Some(f);
                    }
                    Err(_) => proxima = Instant::now() + DE_NOVO,
                }
            }
            Acao::Lancar(Tipo::Agente) => {
                let p = perfil
                    .clone()
                    .expect("decidir só lança o agente com perfil");
                match lancar_agente(&exe, sessao, &p) {
                    Ok(f) => filho = Some(f),
                    Err(_) => proxima = Instant::now() + DE_NOVO,
                }
            }
            Acao::EncerrarAgente => {
                if let Some(f) = filho.take() {
                    // SAFETY: processo nosso; o handle fecha no `Drop` depois de esperar.
                    unsafe {
                        TerminateProcess(f.processo, 0);
                        WaitForSingleObject(f.processo, 5_000);
                    }
                }
                pausado = false;
                proxima = Instant::now();
                if let Some(t) = token {
                    // SAFETY: token de `token_do_usuario`, não usado nesta volta.
                    unsafe { CloseHandle(t) };
                }
                continue;
            }
            Acao::Nada => {
                if let Some(t) = token {
                    // SAFETY: idem.
                    unsafe { CloseHandle(t) };
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
                let tipo = filho.as_ref().map(|f| f.tipo);
                if let Some(f) = filho.take() {
                    // SAFETY: processo terminado, handle ainda aberto.
                    unsafe { GetExitCodeProcess(f.processo, &mut codigo) };
                }
                // Só o app tem "Sair" na bandeja; o agente que sai volta depois da espera.
                pausado = tipo == Some(Tipo::App) && codigo == 0;
                proxima = Instant::now() + DE_NOVO;
            }
            _ => {}
        }
    }
    if let Some(f) = filho.take().filter(|f| f.tipo == Tipo::Agente) {
        // SAFETY: processo nosso; o serviço parando leva o agente junto.
        unsafe { TerminateProcess(f.processo, 0) };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_com_usuario_agente_sem_ninguem() {
        // Ninguém logado e um perfil conhecido: agente.
        assert_eq!(
            decidir(None, false, false, true, true),
            Acao::Lancar(Tipo::Agente)
        );
        // Sem perfil (serviço novo, ninguém entrou ainda): espera.
        assert_eq!(decidir(None, false, false, true, false), Acao::Nada);
        // Alguém logado: o app; saiu pela bandeja: nada até a próxima entrada.
        assert_eq!(
            decidir(None, true, false, true, true),
            Acao::Lancar(Tipo::App)
        );
        assert_eq!(decidir(None, true, true, true, true), Acao::Nada);
        // Dentro da espera depois de uma falha: nada.
        assert_eq!(decidir(None, true, false, false, true), Acao::Nada);
        // Alguém entrou com o agente aberto: encerra o agente.
        assert_eq!(
            decidir(Some(Tipo::Agente), true, false, true, true),
            Acao::EncerrarAgente
        );
        assert_eq!(
            decidir(Some(Tipo::Agente), false, false, true, true),
            Acao::Nada
        );
        assert_eq!(
            decidir(Some(Tipo::App), true, false, true, true),
            Acao::Nada
        );
    }
}
