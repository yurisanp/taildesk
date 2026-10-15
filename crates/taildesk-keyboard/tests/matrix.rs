//! Matriz de testes de `11-teclado.md` §5, de ponta a ponta e sem SO:
//! teclas físicas no controlador -> [`Controller`] -> fio -> [`Host`] -> um SO simulado com o
//! layout do host, que diz o texto e os atalhos que o usuário veria do outro lado.

use std::collections::BTreeSet;

use taildesk_keyboard::fixtures::{abnt2, us, us_intl_windows, xkb_br_abnt2, xkb_us, xkb_us_intl};
use taildesk_keyboard::{
    Controller, Hid, Host, Inject, KeyMsg, Keymap, Level, Locks, Mode, Mods, Output, ReverseMap,
    Translation, effective_translation, is_modifier, is_non_printing, keypad_nav, layout_tag,
};

const L_CTRL: Hid = 0xE0;
const L_SHIFT: Hid = 0xE1;
const L_ALT: Hid = 0xE2;
const L_META: Hid = 0xE3;
const R_ALT: Hid = 0xE6;
const CAPS: Hid = 0x39;
const NUM: Hid = 0x53;

/// SO do host simulado: aplica as teclas injetadas no layout, com teclas mortas, Caps Lock e
/// atalhos, como o Windows faria.
struct HostOs {
    keymap: Keymap,
    has_altgr: bool,
    held: BTreeSet<Hid>,
    locks: Locks,
    pending: Option<char>,
    text: String,
    events: Vec<String>,
}

impl HostOs {
    fn new(keymap: Keymap) -> Self {
        let has_altgr = keymap
            .keys
            .iter()
            .any(|&(_, l, _)| matches!(l, Level::AltGr | Level::ShiftAltGr));
        HostOs {
            keymap,
            has_altgr,
            held: BTreeSet::new(),
            locks: Locks::default(),
            pending: None,
            text: String::new(),
            events: Vec::new(),
        }
    }

    fn mods(&self) -> Mods {
        let mut m = Mods::NONE;
        for &h in &self.held {
            m = m.with(match h {
                0xE0 | 0xE4 => Mods::CTRL,
                0xE1 | 0xE5 => Mods::SHIFT,
                0xE2 => Mods::ALT,
                0xE6 if self.has_altgr => Mods::ALTGR,
                0xE6 => Mods::ALT,
                0xE3 | 0xE7 => Mods::META,
                _ => Mods::NONE,
            });
        }
        m
    }

    fn inject(&mut self, i: Inject) {
        let (hid, down) = match i {
            Inject::Unicode(c) => {
                self.text.push(c);
                return;
            }
            Inject::Key { hid, down } => (hid, down),
        };
        if !down {
            self.held.remove(&hid);
            return;
        }
        if is_modifier(hid) {
            self.held.insert(hid);
            return;
        }
        match hid {
            CAPS => self.locks.caps = !self.locks.caps,
            NUM => self.locks.num = !self.locks.num,
            _ => {}
        }
        if matches!(hid, CAPS | NUM) {
            return;
        }
        let hid = match keypad_nav(hid) {
            Some(nav) if !self.locks.num => nav,
            _ => hid,
        };
        let mods = self.mods();
        let shortcut = mods.intersects(Mods::CTRL.with(Mods::ALT).with(Mods::META));
        if is_non_printing(hid) {
            self.events.push(format!("{}<{hid:#04x}>", mod_names(mods)));
            return;
        }
        if shortcut {
            let base = match self.keymap.output(hid, Level::Base) {
                Some(Output::Char(c)) => c.to_string(),
                _ => format!("<{hid:#04x}>"),
            };
            self.events.push(format!("{}{base}", mod_names(mods)));
            return;
        }
        let mut level = Level::from_mods(mods);
        if self.locks.caps && self.keymap.caps_affects(hid) {
            level = level.caps_flipped();
        }
        let alone = |km: &Keymap, d: char| km.compose(d, ' ').unwrap_or(d);
        match (self.pending.take(), self.keymap.output(hid, level)) {
            (p, None) => self.pending = p,
            (None, Some(Output::Char(c))) => self.text.push(c),
            (None, Some(Output::Dead(d))) => self.pending = Some(d),
            (Some(p), Some(Output::Char(c))) => match self.keymap.compose(p, c) {
                Some(r) => self.text.push(r),
                None => {
                    self.text.push(alone(&self.keymap, p));
                    self.text.push(c);
                }
            },
            (Some(p), Some(Output::Dead(d))) => match self.keymap.compose(p, d) {
                Some(r) => self.text.push(r),
                None => {
                    self.text.push(alone(&self.keymap, p));
                    self.text.push(alone(&self.keymap, d));
                }
            },
        }
    }
}

fn mod_names(m: Mods) -> String {
    let mut s = String::new();
    for (bit, name) in [
        (Mods::CTRL, "Ctrl+"),
        (Mods::ALT, "Alt+"),
        (Mods::META, "Meta+"),
        (Mods::SHIFT, "Shift+"),
    ] {
        if m.contains(bit) {
            s.push_str(name);
        }
    }
    s
}

/// Uma sessão: o teclado do controlador (estado dos modificadores físicos) ligado ao host.
struct Session {
    local: Keymap,
    controller: Controller,
    host: Host,
    os: HostOs,
    mods: Mods,
    wire: Vec<KeyMsg>,
}

impl Session {
    fn new(local: Keymap, remote: Keymap) -> Self {
        let tr = effective_translation(
            Mode::Auto,
            &layout_tag(&local.name),
            &layout_tag(&remote.name),
        );
        Self::with(local, remote, tr)
    }

    fn with(local: Keymap, remote: Keymap, tr: Translation) -> Self {
        Session {
            controller: Controller::new(local.clone(), tr),
            local,
            host: Host::new(remote.clone()),
            os: HostOs::new(remote),
            mods: Mods::NONE,
            wire: Vec::new(),
        }
    }

    fn deliver(&mut self, msgs: Vec<KeyMsg>) {
        let mut inject = Vec::new();
        for m in &msgs {
            inject.clear();
            self.host.apply(m, self.os.locks, &mut inject);
            for &i in &inject {
                self.os.inject(i);
            }
        }
        self.wire.extend(msgs);
    }

    fn event(&mut self, hid: Hid, down: bool) {
        let bit = match hid {
            L_CTRL | 0xE4 => Mods::CTRL,
            L_SHIFT | 0xE5 => Mods::SHIFT,
            L_ALT => Mods::ALT,
            L_META | 0xE7 => Mods::META,
            R_ALT => Mods::ALTGR,
            _ => Mods::NONE,
        };
        if down {
            self.mods = self.mods.with(bit);
        } else {
            self.mods = without(self.mods, bit);
        }
        let mut out = Vec::new();
        self.controller.key(hid, down, self.mods, &mut out);
        self.deliver(out);
    }

    fn down(&mut self, hid: Hid) {
        self.event(hid, true);
    }

    fn up(&mut self, hid: Hid) {
        self.event(hid, false);
    }

    fn tap(&mut self, hid: Hid) {
        self.down(hid);
        self.up(hid);
    }

    /// Aperta `keys` em ordem (modificadores primeiro) e solta ao contrário.
    fn chord(&mut self, keys: &[Hid]) {
        for &k in keys {
            self.down(k);
        }
        for &k in keys.iter().rev() {
            self.up(k);
        }
    }

    /// Digita `text` no teclado do controlador como uma pessoa faria nesse layout.
    /// Devolve `false` se algum caractere não existe no layout do controlador.
    fn type_text(&mut self, text: &str) -> bool {
        let rm = ReverseMap::build(&self.local);
        for c in text.chars() {
            let Some(seq) = rm.lookup(c) else {
                return false;
            };
            for s in seq.iter().copied() {
                let mods: &[Hid] = match s.level {
                    Level::Base => &[],
                    Level::Shift => &[L_SHIFT],
                    Level::AltGr => &[R_ALT],
                    Level::ShiftAltGr => &[L_SHIFT, R_ALT],
                };
                let mut keys = mods.to_vec();
                keys.push(s.hid);
                self.chord(&keys);
            }
        }
        true
    }

    fn focus_lost(&mut self) {
        let mut out = Vec::new();
        self.controller.focus_lost(&mut out);
        self.mods = Mods::NONE;
        self.deliver(out);
    }
}

fn without(m: Mods, bit: Mods) -> Mods {
    let mut r = Mods::NONE;
    for b in [Mods::SHIFT, Mods::CTRL, Mods::ALT, Mods::META, Mods::ALTGR] {
        if m.contains(b) && b != bit {
            r = r.with(b);
        }
    }
    r
}

/// Os pares obrigatórios de §5. Os lados Linux usam o dump real do libxkbcommon.
fn pairs() -> Vec<(&'static str, Keymap, Keymap)> {
    vec![
        (
            "Win US-Intl -> Linux ABNT2",
            us_intl_windows(),
            xkb_br_abnt2(),
        ),
        (
            "Linux ABNT2 -> Win US-Intl",
            xkb_br_abnt2(),
            us_intl_windows(),
        ),
        ("Win ABNT2 -> Linux US", abnt2(), xkb_us()),
        ("Linux US -> Win ABNT2", xkb_us(), abnt2()),
        // ABNT2 dos dois lados: Físico só no mesmo SO (as mortas sozinhas diferem).
        ("Win ABNT2 -> Win ABNT2", abnt2(), abnt2()),
        ("Linux ABNT2 -> Linux ABNT2", xkb_br_abnt2(), xkb_br_abnt2()),
        ("Win ABNT2 -> Linux ABNT2", abnt2(), xkb_br_abnt2()),
        ("Linux ABNT2 -> Win ABNT2", xkb_br_abnt2(), abnt2()),
        ("Linux US -> Win US-Intl", xkb_us(), us_intl_windows()),
        ("Win US-Intl -> Linux US", us_intl_windows(), xkb_us()),
        (
            "Linux US-Intl -> Linux ABNT2",
            xkb_us_intl(),
            xkb_br_abnt2(),
        ),
        (
            "Linux ABNT2 -> Linux US-Intl",
            xkb_br_abnt2(),
            xkb_us_intl(),
        ),
    ]
}

const ACCENTED: &str = "çÇáéíóúàâêôãõüÁÉÍÓÚÀÂÊÔÃÕÜ";
const SYMBOLS: &str = "/?;:[]{}\\|";
const ACCENTS_ALONE: &str = "'\"`~^´¨";

#[test]
fn text_arrives_unchanged_in_every_pair() {
    for (name, local, remote) in pairs() {
        for text in [
            ACCENTED,
            SYMBOLS,
            ACCENTS_ALONE,
            "Olá, mundo! 'tá' \"x\" ~a",
        ] {
            // Só o que o controlador consegue digitar: US não tem `ç`, e assim por diante.
            let typable: String = text
                .chars()
                .filter(|&c| ReverseMap::build(&local).lookup(c).is_some())
                .collect();
            let mut s = Session::new(local.clone(), remote.clone());
            assert!(s.type_text(&typable));
            assert_eq!(s.os.text, typable, "{name}: {text:?}");
            assert!(s.os.events.is_empty(), "{name}: {:?}", s.os.events);
            assert!(s.host.held().next().is_none(), "{name}: tecla presa");
        }
    }
}

#[test]
fn equal_layouts_go_physical() {
    let mut s = Session::new(abnt2(), abnt2());
    assert_eq!(s.controller.translation(), Translation::Physical);
    s.type_text("é");
    assert!(
        s.wire.iter().all(|m| matches!(m, KeyMsg::Key { .. })),
        "{:?}",
        s.wire
    );
    assert_eq!(s.os.text, "é");
    let s = Session::new(us_intl_windows(), abnt2());
    assert_eq!(s.controller.translation(), Translation::Hybrid);
    let s = Session::new(xkb_br_abnt2(), xkb_br_abnt2());
    assert_eq!(s.controller.translation(), Translation::Physical);
    // Windows ABNT2 -> Linux ABNT2 vai pelo híbrido: `´` + espaço dá `'` no Linux.
    let mut s = Session::new(abnt2(), xkb_br_abnt2());
    assert_eq!(s.controller.translation(), Translation::Hybrid);
    s.tap(0x2F);
    s.tap(0x2C);
    assert_eq!(s.os.text, "´");
}

/// Requisito do Yuri: `'` + `c` no Windows US-Intl é `ç` no host ABNT2, e `'t` continua `'t`.
#[test]
fn us_intl_dead_keys_resolved_on_controller() {
    let mut s = Session::new(us_intl_windows(), abnt2());
    s.tap(0x34); // ' (morta)
    s.tap(0x06); // c
    s.tap(0x34);
    s.tap(0x17); // t
    s.tap(0x34);
    s.tap(0x2C); // espaço
    s.chord(&[L_SHIFT, 0x34]); // " (morta)
    s.tap(0x18); // u
    assert_eq!(s.os.text, "ç't'ü");
    // A morta nunca chega "pela metade" ao host.
    assert!(
        s.wire
            .iter()
            .all(|m| !matches!(m, KeyMsg::Char { text, .. } if text.is_empty()))
    );
}

#[test]
fn abnt2_altgr_and_extra_keys() {
    let mut s = Session::new(abnt2(), us_intl_windows());
    s.chord(&[R_ALT, 0x14]); // AltGr+Q = /
    s.chord(&[R_ALT, 0x1A]); // AltGr+W = ?
    s.chord(&[R_ALT, 0x08]); // AltGr+E = °
    s.tap(0x87); // / ao lado do Shift direito
    s.chord(&[L_SHIFT, 0x87]); // ?
    s.tap(0x85); // Keypad Comma = .
    assert_eq!(s.os.text, "/?°/?.");
    // `°` não existe no US-Intl: vai como unicode.
    let mut s = Session::new(abnt2(), us());
    s.tap(0x87);
    s.tap(0x85);
    assert_eq!(s.os.text, "/.");
}

#[test]
fn shortcuts_follow_the_letter() {
    for (name, local, remote) in pairs() {
        let mut s = Session::new(local, remote);
        s.chord(&[L_CTRL, 0x06]); // Ctrl+C
        s.chord(&[L_CTRL, 0x19]); // Ctrl+V
        s.chord(&[L_CTRL, 0x1D]); // Ctrl+Z
        s.chord(&[L_CTRL, L_SHIFT, 0x17]); // Ctrl+Shift+T
        s.chord(&[L_ALT, 0x2B]); // Alt+Tab
        s.chord(&[L_META, 0x08]); // Win+E
        s.chord(&[L_CTRL, L_ALT, 0x17]); // Ctrl+Alt+T (terminal no Linux)
        assert_eq!(
            s.os.events,
            [
                "Ctrl+c",
                "Ctrl+v",
                "Ctrl+z",
                "Ctrl+Shift+t",
                "Alt+<0x2b>",
                "Meta+e",
                "Ctrl+Alt+t"
            ],
            "{name}"
        );
        assert!(s.os.text.is_empty(), "{name}: {:?}", s.os.text);
        assert!(s.host.held().next().is_none(), "{name}");
    }
}

#[test]
fn shortcut_with_letter_missing_on_host_uses_position() {
    // Ctrl+ç num host US: não há `ç`, vai a posição física (0x33 = `;` no US).
    let mut s = Session::new(abnt2(), us());
    s.chord(&[L_CTRL, 0x33]);
    assert_eq!(s.os.events, ["Ctrl+;"]);
}

#[test]
fn numpad_with_and_without_num_lock() {
    for (name, local, remote) in pairs() {
        let mut s = Session::new(local, remote);
        // Num Lock ligado no controlador, desligado no host: chega o dígito.
        s.controller.set_locks(Locks {
            caps: false,
            num: true,
        });
        if s.controller.translation() == Translation::Physical {
            s.os.locks.num = true;
        }
        s.tap(0x59); // 1
        s.tap(0x57); // +
        // Desligado no controlador e ligado no host: chega End, não `1`.
        s.controller.set_locks(Locks::default());
        s.os.locks.num = true;
        s.tap(0x59);
        assert_eq!(s.os.text, "1+", "{name}");
        assert_eq!(s.os.events, ["<0x4d>"], "{name}");
    }
}

#[test]
fn caps_lock_on_one_side_only() {
    for (name, local, remote) in pairs() {
        // Caps ligado só no host: o texto não pode sair invertido.
        let mut s = Session::new(local.clone(), remote.clone());
        s.os.locks.caps = true;
        s.type_text("aB");
        if s.controller.translation() == Translation::Physical {
            // Físico sincroniza o lock do controlador (desligado) antes da tecla.
            assert!(!s.os.locks.caps, "{name}");
        }
        assert_eq!(s.os.text, "aB", "{name}");

        // Caps ligado só no controlador: sai maiúscula, como o usuário veria localmente.
        let mut s = Session::new(local, remote);
        s.tap(CAPS);
        s.tap(0x04);
        s.chord(&[L_SHIFT, 0x05]);
        assert_eq!(s.os.text, "Ab", "{name}");
    }
}

#[test]
fn layout_change_mid_session() {
    let mut s = Session::new(us_intl_windows(), abnt2());
    s.tap(0x33);
    // O usuário troca o layout do Windows para ABNT2: a mesma tecla passa a ser `ç`.
    let mut out = Vec::new();
    s.controller.set_keymap(abnt2(), &mut out);
    s.local = abnt2();
    s.deliver(out);
    s.tap(0x33);
    assert_eq!(s.os.text, ";ç");

    // E o host troca de layout: refaz a busca reversa.
    s.host.set_keymap(us());
    s.os = HostOs::new(us());
    s.tap(0x34); // ~ (morta) no ABNT2
    s.tap(0x04); // a
    s.type_text(";");
    assert_eq!(s.os.text, "ã;");
}

#[test]
fn key_repeat_and_focus_loss() {
    let mut s = Session::new(us_intl_windows(), abnt2());
    // Repetição do SO do controlador: três `down` sem `up`.
    s.down(0x04);
    s.down(0x04);
    s.down(0x04);
    s.up(0x04);
    assert_eq!(s.os.text, "aaa");

    // Shift apertado e a janela perde o foco: o host solta tudo.
    s.down(L_SHIFT);
    assert!(s.os.held.contains(&L_SHIFT));
    s.focus_lost();
    assert!(s.os.held.is_empty());
    assert!(s.host.held().next().is_none());
    // O `up` do Shift que chega depois não faz nada no host.
    s.up(L_SHIFT);
    s.tap(0x04);
    assert_eq!(s.os.text, "aaaa");
}

#[test]
fn shift_held_does_not_leak_into_text() {
    // O Shift vai como tecla física (Shift+clique precisa dele), mas não pode mudar o caractere
    // que já veio resolvido.
    let mut s = Session::new(us_intl_windows(), abnt2());
    s.down(L_SHIFT);
    s.tap(0x33); // `:` no US
    s.tap(0x34); // `"` (morta) no US-Intl
    s.tap(0x2C); // espaço
    s.up(L_SHIFT);
    assert_eq!(s.os.text, ":\"");
}

#[test]
fn physical_mode_forced() {
    // Jogos: tudo por posição, mesmo com layouts diferentes.
    let mut s = Session::with(us_intl_windows(), abnt2(), Translation::Physical);
    s.tap(0x33);
    assert_eq!(s.os.text, "ç");
    assert!(s.wire.iter().all(|m| matches!(m, KeyMsg::Key { .. })));
}
