//! Leitura do layout no Linux com o libxkbcommon (`11-teclado.md` §2.1).
//!
//! O libxkbcommon é carregado em tempo de execução (`xkbcommon-dl`, o mesmo que o winit usa),
//! então o binário não exige o pacote `-dev` para compilar. No Wayland o keymap vem do
//! compositor (libei/portal) como texto e entra por [`ler_keymap_texto`]; no X11 e em testes,
//! por nomes RMLVO em [`ler_keymap_nomes`].

use std::ffi::{CString, c_char};
use std::ptr;

use taildesk_keyboard::codes::{hid_to_evdev, known_hids};
use taildesk_keyboard::{Hid, Keymap, Level, Output, is_keypad};
use xkbcommon_dl::{
    XkbCommon, XkbCommonCompose, xkb_compose_compile_flags, xkb_compose_feed_result,
    xkb_compose_state_flags, xkb_compose_status, xkb_context, xkb_context_flags, xkb_keymap,
    xkb_keymap_compile_flags, xkb_keymap_format, xkb_keysym_t, xkb_rule_names,
};

/// Erro ao ler o layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErroXkb {
    /// `libxkbcommon.so.0` não está instalado.
    SemBiblioteca,
    /// O libxkbcommon não conseguiu montar o keymap (layout inexistente, sem xkeyboard-config).
    KeymapInvalido,
}

impl std::fmt::Display for ErroXkb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErroXkb::SemBiblioteca => f.write_str("libxkbcommon não encontrado"),
            ErroXkb::KeymapInvalido => f.write_str("keymap XKB inválido"),
        }
    }
}

impl std::error::Error for ErroXkb {}

/// Contexto e keymap do libxkbcommon, liberados no `Drop`.
struct Guarda {
    xkb: &'static XkbCommon,
    ctx: *mut xkb_context,
    keymap: *mut xkb_keymap,
}

impl Drop for Guarda {
    fn drop(&mut self) {
        // SAFETY: ponteiros criados pelo próprio libxkbcommon e liberados uma única vez aqui;
        // `unref` aceita nulo.
        unsafe {
            (self.xkb.xkb_keymap_unref)(self.keymap);
            (self.xkb.xkb_context_unref)(self.ctx);
        }
    }
}

fn contexto() -> Result<(&'static XkbCommon, *mut xkb_context), ErroXkb> {
    let xkb = xkbcommon_dl::xkbcommon_option().ok_or(ErroXkb::SemBiblioteca)?;
    // SAFETY: função do libxkbcommon sem pré-condições; o resultado é conferido.
    let ctx = unsafe { (xkb.xkb_context_new)(xkb_context_flags::XKB_CONTEXT_NO_FLAGS) };
    if ctx.is_null() {
        return Err(ErroXkb::KeymapInvalido);
    }
    Ok((xkb, ctx))
}

/// Lê um layout pelos nomes XKB (ex.: `"br"`, `"abnt2"` ou `"us"`, `"intl"`), com as regras
/// padrão do xkeyboard-config. `locale` escolhe a tabela Compose das teclas mortas.
pub fn ler_keymap_nomes(layout: &str, variante: &str, locale: &str) -> Result<Keymap, ErroXkb> {
    let (xkb, ctx) = contexto()?;
    let layout_c = CString::new(layout).map_err(|_| ErroXkb::KeymapInvalido)?;
    let variante_c = CString::new(variante).map_err(|_| ErroXkb::KeymapInvalido)?;
    let nomes = xkb_rule_names {
        rules: ptr::null(),
        model: ptr::null(),
        layout: layout_c.as_ptr(),
        variant: variante_c.as_ptr(),
        options: ptr::null(),
    };
    // SAFETY: `ctx` é válido e `nomes` aponta para strings vivas durante a chamada.
    let keymap = unsafe {
        (xkb.xkb_keymap_new_from_names)(
            ctx,
            &nomes,
            xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
        )
    };
    let guarda = Guarda { xkb, ctx, keymap };
    if keymap.is_null() {
        return Err(ErroXkb::KeymapInvalido);
    }
    let nome = if variante.is_empty() {
        format!("xkb:{layout}")
    } else {
        format!("xkb:{layout}({variante})")
    };
    Ok(ler(&guarda, nome, 0, locale))
}

/// Lê o keymap em texto que o compositor entrega (formato XKB v1), no grupo `grupo`.
pub fn ler_keymap_texto(texto: &str, grupo: u32, locale: &str) -> Result<Keymap, ErroXkb> {
    let (xkb, ctx) = contexto()?;
    let texto_c = CString::new(texto).map_err(|_| ErroXkb::KeymapInvalido)?;
    // SAFETY: `ctx` é válido e o texto é uma string C viva durante a chamada.
    let keymap = unsafe {
        (xkb.xkb_keymap_new_from_string)(
            ctx,
            texto_c.as_ptr(),
            xkb_keymap_format::XKB_KEYMAP_FORMAT_TEXT_V1,
            xkb_keymap_compile_flags::XKB_KEYMAP_COMPILE_NO_FLAGS,
        )
    };
    let guarda = Guarda { xkb, ctx, keymap };
    if keymap.is_null() {
        return Err(ErroXkb::KeymapInvalido);
    }
    let nome = nome_dos_simbolos(texto).unwrap_or_else(|| "xkb".into());
    Ok(ler(&guarda, nome, grupo, locale))
}

/// Nome do keymap a partir da linha `xkb_symbols "pc+br(abnt2)+inc(evdev)"` do texto:
/// `xkb:br(abnt2)`. Só o primeiro layout depois de `pc`.
fn nome_dos_simbolos(texto: &str) -> Option<String> {
    let linha = texto
        .lines()
        .find(|l| l.trim_start().starts_with("xkb_symbols"))?;
    let dentro = linha.split('"').nth(1)?;
    let layout = dentro
        .split('+')
        .find(|p| !p.is_empty() && !p.starts_with("pc") && !p.starts_with("inet"))?;
    Some(format!(
        "xkb:{}",
        layout.split(':').next().unwrap_or(layout)
    ))
}

/// Locale da sessão para a tabela Compose (`LC_ALL` > `LC_CTYPE` > `LANG`).
pub fn locale_da_sessao() -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "C".into())
}

/// Layout e variante ativos da sessão, do que o ambiente gravou: KDE (`kxkbrc`), GNOME
/// (`org.gnome.desktop.input-sources`) ou o padrão do sistema (`localectl`). No Wayland não há
/// API comum para isso; o primeiro da lista vale (troca de layout no meio da sessão ainda não é
/// acompanhada).
pub fn layout_da_sessao() -> Option<(String, String)> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")));
    if let Some(dir) = config
        && let Ok(texto) = std::fs::read_to_string(dir.join("kxkbrc"))
        && let Some(l) = do_kxkbrc(&texto)
    {
        return Some(l);
    }
    let gsettings = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.input-sources", "sources"])
        .output();
    if let Ok(saida) = gsettings
        && let Some(l) = do_gsettings(&String::from_utf8_lossy(&saida.stdout))
    {
        return Some(l);
    }
    let localectl = std::process::Command::new("localectl")
        .arg("status")
        .output();
    if let Ok(saida) = localectl {
        return do_localectl(&String::from_utf8_lossy(&saida.stdout));
    }
    None
}

fn primeiro(lista: &str) -> &str {
    lista.split(',').next().unwrap_or("").trim()
}

/// `LayoutList=br,us` e `VariantList=abnt2,` em `[Layout]`.
fn do_kxkbrc(texto: &str) -> Option<(String, String)> {
    let valor = |chave: &str| {
        texto
            .lines()
            .find_map(|l| l.trim().strip_prefix(chave)?.strip_prefix('='))
            .map(primeiro)
    };
    let layout = valor("LayoutList").filter(|l| !l.is_empty())?;
    Some((
        layout.to_owned(),
        valor("VariantList").unwrap_or("").to_owned(),
    ))
}

/// `[('xkb', 'br+abnt2'), ('xkb', 'us')]`.
fn do_gsettings(texto: &str) -> Option<(String, String)> {
    let i = texto.find("('xkb', '")? + "('xkb', '".len();
    let fonte = texto[i..].split('\'').next()?;
    let (layout, variante) = fonte.split_once('+').unwrap_or((fonte, ""));
    (!layout.is_empty()).then(|| (layout.to_owned(), variante.to_owned()))
}

/// `X11 Layout: br` e `X11 Variant: abnt2`.
fn do_localectl(texto: &str) -> Option<(String, String)> {
    let valor = |chave: &str| {
        texto
            .lines()
            .find_map(|l| l.trim().strip_prefix(chave))
            .map(|v| primeiro(v.trim()))
    };
    let layout = valor("X11 Layout:").filter(|l| !l.is_empty() && *l != "n/a")?;
    Some((
        layout.to_owned(),
        valor("X11 Variant:").unwrap_or("").to_owned(),
    ))
}

/// Acento "solto" de uma tecla morta do XKB (`dead_acute` -> `´`).
fn acento(sym: xkb_keysym_t) -> Option<char> {
    Some(match sym {
        0xFE50 => '`',
        0xFE51 => '´',
        0xFE52 => '^',
        0xFE53 => '~',
        0xFE54 => '¯',
        0xFE55 => '˘',
        0xFE56 => '˙',
        0xFE57 => '¨',
        0xFE58 => '˚',
        0xFE59 => '˝',
        0xFE5A => 'ˇ',
        0xFE5B => '¸',
        0xFE5C => '˛',
        _ => return None,
    })
}

fn ler(g: &Guarda, nome: String, grupo: u32, locale: &str) -> Keymap {
    let xkb = g.xkb;
    let mod_idx = |nome: &[u8]| {
        // SAFETY: keymap válido e `nome` termina em NUL.
        let i = unsafe { (xkb.xkb_keymap_mod_get_index)(g.keymap, nome.as_ptr().cast::<c_char>()) };
        if i == xkbcommon_dl::XKB_MOD_INVALID {
            0
        } else {
            1u32 << i
        }
    };
    let shift = mod_idx(b"Shift\0");
    let num = mod_idx(b"Mod2\0");
    // AltGr é o nível 3 (`ISO_Level3_Shift`), ligado ao Mod5 em todo layout do xkeyboard-config.
    let altgr = mod_idx(b"Mod5\0");

    // SAFETY: keymap válido; o estado é liberado no fim desta função.
    let estado = unsafe { (xkb.xkb_state_new)(g.keymap) };
    let mut keys = Vec::new();
    // Tecla morta -> keysym; caractere -> keysym, para alimentar a tabela Compose.
    let mut mortas: Vec<(char, xkb_keysym_t)> = Vec::new();
    let mut bases: Vec<(char, xkb_keysym_t)> = Vec::new();
    if !estado.is_null() {
        for level in [Level::Base, Level::Shift, Level::AltGr, Level::ShiftAltGr] {
            let nivel = match level {
                Level::Base => 0,
                Level::Shift => shift,
                Level::AltGr => altgr,
                Level::ShiftAltGr => shift | altgr,
            };
            if nivel != 0 && level != Level::Shift && altgr == 0 {
                continue;
            }
            for hid in known_hids() {
                let Some(evdev) = hid_to_evdev(hid) else {
                    continue;
                };
                // 0x32 e 0x31 são a mesma tecla física no Linux: grava só a primeira.
                if hid == 0x32 {
                    continue;
                }
                // Teclado numérico lido com o Num Lock ligado.
                let mods = if is_keypad(hid) { nivel | num } else { nivel };
                let keycode = u32::from(evdev) + 8;
                // SAFETY: estado válido; keycode fora do keymap devolve NoSymbol (0).
                let sym = unsafe {
                    (xkb.xkb_state_update_mask)(estado, mods, 0, 0, 0, 0, grupo);
                    (xkb.xkb_state_key_get_one_sym)(estado, keycode)
                };
                if let Some(saida) = saida(xkb, hid, sym) {
                    match saida {
                        Output::Dead(c) => {
                            if !mortas.iter().any(|&(m, _)| m == c) {
                                mortas.push((c, sym));
                            }
                        }
                        Output::Char(c) => {
                            if !bases.iter().any(|&(b, _)| b == c) {
                                bases.push((c, sym));
                            }
                        }
                    }
                    keys.push((hid, level, saida));
                }
            }
        }
        // SAFETY: criado acima e liberado uma vez.
        unsafe { (xkb.xkb_state_unref)(estado) };
    }
    let compose = xkbcommon_dl::xkbcommon_compose_option()
        .map(|c| composicoes(c, g.ctx, locale, &mortas, &bases))
        .unwrap_or_default();
    Keymap {
        name: nome,
        keys,
        compose,
    }
}

fn saida(xkb: &XkbCommon, hid: Hid, sym: xkb_keysym_t) -> Option<Output> {
    if sym == 0 {
        return None;
    }
    if let Some(c) = acento(sym) {
        return Some(Output::Dead(c));
    }
    // Teclas que não produzem texto nunca entram, mesmo que o keysym tenha código (Enter, Tab).
    if taildesk_keyboard::is_non_printing(hid) {
        return None;
    }
    // SAFETY: função pura do libxkbcommon.
    let u = unsafe { (xkb.xkb_keysym_to_utf32)(sym) };
    let c = char::from_u32(u)?;
    (u != 0 && !c.is_control()).then_some(Output::Char(c))
}

/// Tabela de composição: cada morta com cada caractere do layout, com espaço e com outra morta.
fn composicoes(
    c: &XkbCommonCompose,
    ctx: *mut xkb_context,
    locale: &str,
    mortas: &[(char, xkb_keysym_t)],
    bases: &[(char, xkb_keysym_t)],
) -> Vec<(char, char, char)> {
    let Ok(locale_c) = CString::new(locale) else {
        return Vec::new();
    };
    // SAFETY: `ctx` é válido; tabela e estado são liberados no fim.
    let tabela = unsafe {
        (c.xkb_compose_table_new_from_locale)(
            ctx,
            locale_c.as_ptr(),
            xkb_compose_compile_flags::XKB_COMPOSE_COMPILE_NO_FLAGS,
        )
    };
    if tabela.is_null() {
        return Vec::new();
    }
    // SAFETY: tabela válida.
    let estado = unsafe {
        (c.xkb_compose_state_new)(tabela, xkb_compose_state_flags::XKB_COMPOSE_STATE_NO_FLAGS)
    };
    let mut out = Vec::new();
    if !estado.is_null() {
        let segundas = bases.iter().chain(mortas.iter());
        for &(m, msym) in mortas {
            for &(b, bsym) in segundas.clone() {
                // SAFETY: estado válido; o buffer tem o tamanho informado.
                let r = unsafe {
                    (c.xkb_compose_state_reset)(estado);
                    let a = (c.xkb_compose_state_feed)(estado, msym);
                    let b = (c.xkb_compose_state_feed)(estado, bsym);
                    if a != xkb_compose_feed_result::XKB_COMPOSE_FEED_ACCEPTED
                        || b != xkb_compose_feed_result::XKB_COMPOSE_FEED_ACCEPTED
                        || (c.xkb_compose_state_get_status)(estado)
                            != xkb_compose_status::XKB_COMPOSE_COMPOSED
                    {
                        None
                    } else {
                        let mut buf = [0u8; 16];
                        let n = (c.xkb_compose_state_get_utf8)(
                            estado,
                            buf.as_mut_ptr().cast::<c_char>(),
                            buf.len(),
                        );
                        usize::try_from(n)
                            .ok()
                            .filter(|&n| n < buf.len())
                            .and_then(|n| std::str::from_utf8(&buf[..n]).ok())
                            .and_then(|s| {
                                let mut it = s.chars();
                                it.next().filter(|_| it.next().is_none())
                            })
                    }
                };
                if let Some(r) = r {
                    out.push((m, b, r));
                }
            }
        }
        // SAFETY: criado acima.
        unsafe { (c.xkb_compose_state_unref)(estado) };
    }
    // SAFETY: criada acima.
    unsafe { (c.xkb_compose_table_unref)(tabela) };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_das_configuracoes() {
        let par = |l: &str, v: &str| Some((l.to_owned(), v.to_owned()));
        let kde = "[Layout]\nDisplayNames=,\nLayoutList=br,us\nUse=true\nVariantList=abnt2,intl\n";
        assert_eq!(do_kxkbrc(kde), par("br", "abnt2"));
        assert_eq!(do_kxkbrc("[Layout]\nUse=false\n"), None);
        assert_eq!(
            do_gsettings("[('xkb', 'us+intl'), ('xkb', 'br')]\n"),
            par("us", "intl")
        );
        assert_eq!(do_gsettings("[('xkb', 'br')]"), par("br", ""));
        assert_eq!(do_gsettings("@a(ss) []"), None);
        let lc = "   System Locale: LANG=pt_BR.UTF-8\n       X11 Layout: br\n      X11 Model: pc105\n    X11 Variant: abnt2\n";
        assert_eq!(do_localectl(lc), par("br", "abnt2"));
        assert_eq!(do_localectl("   X11 Layout: n/a\n"), None);
    }
    use taildesk_keyboard::ReverseMap;

    /// Só roda onde há libxkbcommon e xkeyboard-config (desktops e o CI do Ubuntu).
    fn ler_ou_pular(layout: &str, variante: &str) -> Option<Keymap> {
        match ler_keymap_nomes(layout, variante, "en_US.UTF-8") {
            Ok(km) => Some(km),
            Err(e) => {
                eprintln!("pulando: {e}");
                None
            }
        }
    }

    #[test]
    fn abnt2_do_xkb() {
        let Some(km) = ler_ou_pular("br", "abnt2") else {
            return;
        };
        assert_eq!(km.name, "xkb:br(abnt2)");
        assert_eq!(km.output(0x33, Level::Base), Some(Output::Char('ç')));
        assert_eq!(km.output(0x2F, Level::Base), Some(Output::Dead('´')));
        assert_eq!(km.output(0x34, Level::Base), Some(Output::Dead('~')));
        assert_eq!(km.output(0x87, Level::Base), Some(Output::Char('/')));
        assert_eq!(km.output(0x14, Level::AltGr), Some(Output::Char('/')));
        assert_eq!(km.output(0x59, Level::Base), Some(Output::Char('1')));
        let rm = ReverseMap::build(&km);
        for c in "çÇáéíóúàâêôãõüÁÉÍÓÚÀÂÊÔÃÕÜ'\"`~^´¨/?;:[]{}\\|°".chars()
        {
            assert!(rm.lookup(c).is_some(), "{c}");
        }
        assert_eq!(km.compose('´', 'e'), Some('é'));
        assert_eq!(km.compose('~', 'a'), Some('ã'));
    }

    #[test]
    fn us_intl_do_xkb() {
        let Some(km) = ler_ou_pular("us", "intl") else {
            return;
        };
        // No Linux o `'` do US-Intl é `dead_acute`, e morta + espaço dá o apóstrofo.
        assert_eq!(km.output(0x34, Level::Base), Some(Output::Dead('´')));
        assert_eq!(km.compose('´', ' '), Some('\''));
        assert_eq!(km.compose('´', 'e'), Some('é'));
    }

    #[test]
    fn nome_vem_do_xkb_symbols() {
        let t =
            "xkb_keymap {\n  xkb_symbols \"pc+br(abnt2)+inet(evdev)+group(alt_shift_toggle)\" {";
        assert_eq!(nome_dos_simbolos(t).as_deref(), Some("xkb:br(abnt2)"));
        assert_eq!(nome_dos_simbolos("nada"), None);
    }

    #[test]
    fn layout_inexistente() {
        if xkbcommon_dl::xkbcommon_option().is_none() {
            return;
        }
        assert_eq!(
            ler_keymap_nomes("nao-existe", "", "C").map(|_| ()),
            Err(ErroXkb::KeymapInvalido)
        );
    }
}
