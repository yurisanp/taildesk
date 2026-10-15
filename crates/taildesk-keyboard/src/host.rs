//! Lado do host: [`KeyMsg`] -> teclas a injetar no layout local (`11-teclado.md` §2.3 e §2.4).

use std::collections::{BTreeSet, HashMap};

use crate::{
    Hid, KeyMsg, Keymap, Level, Locks, Mods, Output, ReverseMap, Stroke, is_keypad, is_modifier,
};

const LEFT_CTRL: Hid = 0xE0;
const LEFT_SHIFT: Hid = 0xE1;
const LEFT_ALT: Hid = 0xE2;
const LEFT_META: Hid = 0xE3;
/// AltGr é o Alt direito nos layouts que têm AltGr.
const RIGHT_ALT: Hid = 0xE6;
const CAPS_LOCK: Hid = 0x39;
const NUM_LOCK: Hid = 0x53;

/// O que o injetor da plataforma deve fazer (SendInput, XTest ou portal/libei).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inject {
    Key {
        hid: Hid,
        down: bool,
    },
    /// Caractere que não existe no layout do host (§2.4 item 4): `KEYEVENTF_UNICODE` no
    /// Windows, keycode remapeado no X11, `NotifyKeyboardKeysym` ou aviso no HUD no Wayland.
    Unicode(char),
}

/// Tradutor do host. Guarda o que está apertado para soltar tudo na perda de foco e para tirar
/// os modificadores do caminho ao digitar um caractere.
#[derive(Debug)]
pub struct Host {
    keymap: Keymap,
    reverse: ReverseMap,
    /// Caractere do nível base -> tecla, para atalhos pela letra.
    shortcut_keys: HashMap<char, Hid>,
    held: BTreeSet<Hid>,
    /// Atalhos apertados: `hid_hint` do controlador -> tecla injetada no host.
    held_shortcuts: HashMap<Hid, Hid>,
}

impl Host {
    pub fn new(keymap: Keymap) -> Self {
        let mut host = Host {
            reverse: ReverseMap::default(),
            shortcut_keys: HashMap::new(),
            keymap: Keymap::default(),
            held: BTreeSet::new(),
            held_shortcuts: HashMap::new(),
        };
        host.set_keymap(keymap);
        host
    }

    /// Troca de layout do host: refaz a busca reversa. O que está apertado continua apertado.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.reverse = ReverseMap::build(&keymap);
        self.shortcut_keys.clear();
        for &(hid, level, out) in &keymap.keys {
            if let (Level::Base, Output::Char(c)) = (level, out) {
                if is_keypad(hid) {
                    continue;
                }
                let c = c.to_lowercase().next().unwrap_or(c);
                let e = self.shortcut_keys.entry(c).or_insert(hid);
                *e = (*e).min(hid);
            }
        }
        self.keymap = keymap;
    }

    /// Teclas apertadas agora (pelo que foi injetado).
    pub fn held(&self) -> impl Iterator<Item = Hid> + '_ {
        self.held.iter().copied()
    }

    /// Traduz uma mensagem. `locks` é o estado atual dos locks do host, lido do SO.
    pub fn apply(&mut self, msg: &KeyMsg, locks: Locks, out: &mut Vec<Inject>) {
        match msg {
            &KeyMsg::Key {
                hid,
                down,
                locks: wanted,
            } => {
                let hid = crate::codes::canonical(hid);
                if let Some(wanted) = wanted {
                    sync_locks(wanted, locks, hid, down, out);
                }
                self.press(hid, down, out);
            }
            &KeyMsg::Shortcut {
                base_char,
                hid_hint,
                mods,
                down,
            } => self.shortcut(base_char, hid_hint, mods, down, out),
            KeyMsg::Char { text, .. } => self.text(text, locks, out),
            KeyMsg::ReleaseAll => self.release_all(out),
        }
    }

    fn press(&mut self, hid: Hid, down: bool, out: &mut Vec<Inject>) {
        if down {
            self.held.insert(hid);
        } else {
            self.held.remove(&hid);
        }
        out.push(Inject::Key { hid, down });
    }

    fn held_mods(&self) -> Mods {
        let mut m = Mods::NONE;
        for &hid in &self.held {
            m = m.with(match hid {
                0xE0 | 0xE4 => Mods::CTRL,
                0xE1 | 0xE5 => Mods::SHIFT,
                0xE2 | 0xE6 => Mods::ALT,
                0xE3 | 0xE7 => Mods::META,
                _ => Mods::NONE,
            });
        }
        m
    }

    fn shortcut(&mut self, base: char, hint: Hid, mods: Mods, down: bool, out: &mut Vec<Inject>) {
        if !down {
            let hid = self.held_shortcuts.remove(&hint).unwrap_or(hint);
            self.press(hid, false, out);
            return;
        }
        // Mesma letra no layout do host; se não existir (Ctrl+ç num host US), a posição física.
        let hid = self.shortcut_keys.get(&base).copied().unwrap_or(hint);
        let have = self.held_mods();
        let added: Vec<Hid> = [
            (Mods::CTRL, LEFT_CTRL),
            (Mods::SHIFT, LEFT_SHIFT),
            (Mods::ALT, LEFT_ALT),
            (Mods::META, LEFT_META),
        ]
        .into_iter()
        .filter(|&(bit, _)| mods.contains(bit) && !have.contains(bit))
        .map(|(_, key)| key)
        .collect();
        for &m in &added {
            out.push(Inject::Key { hid: m, down: true });
        }
        self.held_shortcuts.insert(hint, hid);
        self.press(hid, true, out);
        for &m in added.iter().rev() {
            out.push(Inject::Key {
                hid: m,
                down: false,
            });
        }
    }

    fn text(&mut self, text: &str, locks: Locks, out: &mut Vec<Inject>) {
        // Modificadores que o controlador mandou como tecla física (Shift, AltGr) atrapalham:
        // solta, digita e aperta de novo.
        let mods: Vec<Hid> = self
            .held
            .iter()
            .copied()
            .filter(|&h| is_modifier(h))
            .collect();
        for &m in &mods {
            out.push(Inject::Key {
                hid: m,
                down: false,
            });
        }
        for c in text.chars() {
            match self.reverse.lookup(c) {
                Some(seq) => {
                    for &stroke in seq {
                        self.stroke(stroke, locks, out);
                    }
                }
                None => out.push(Inject::Unicode(c)),
            }
        }
        for &m in &mods {
            out.push(Inject::Key { hid: m, down: true });
        }
    }

    /// Aperta e solta uma tecla com o nível pedido, compensando o Caps Lock do host.
    fn stroke(&self, stroke: Stroke, locks: Locks, out: &mut Vec<Inject>) {
        let level = if locks.caps && self.keymap.caps_affects(stroke.hid) {
            stroke.level.caps_flipped()
        } else {
            stroke.level
        };
        let mods: &[Hid] = match level {
            Level::Base => &[],
            Level::Shift => &[LEFT_SHIFT],
            Level::AltGr => &[RIGHT_ALT],
            Level::ShiftAltGr => &[LEFT_SHIFT, RIGHT_ALT],
        };
        for &m in mods {
            out.push(Inject::Key { hid: m, down: true });
        }
        out.push(Inject::Key {
            hid: stroke.hid,
            down: true,
        });
        out.push(Inject::Key {
            hid: stroke.hid,
            down: false,
        });
        for &m in mods.iter().rev() {
            out.push(Inject::Key {
                hid: m,
                down: false,
            });
        }
    }

    fn release_all(&mut self, out: &mut Vec<Inject>) {
        // Teclas comuns primeiro, modificadores por último (não dispara atalho ao soltar).
        let (mods, keys): (Vec<Hid>, Vec<Hid>) = self.held.iter().partition(|&&h| is_modifier(h));
        for hid in keys.into_iter().chain(mods) {
            out.push(Inject::Key { hid, down: false });
        }
        self.held.clear();
        self.held_shortcuts.clear();
    }
}

/// Modo Físico: deixa Caps e Num Lock do host iguais aos do controlador antes da tecla.
/// A própria tecla de lock não precisa de correção: ela mesma vai alternar o estado.
fn sync_locks(wanted: Locks, have: Locks, hid: Hid, down: bool, out: &mut Vec<Inject>) {
    for (want, has, key) in [
        (wanted.caps, have.caps, CAPS_LOCK),
        (wanted.num, have.num, NUM_LOCK),
    ] {
        // No "apertar" da tecla de lock o controlador já manda o estado alternado; o host chega
        // lá quando a tecla injetada alternar o dele.
        let want = if hid == key && down { !want } else { want };
        if want != has {
            out.push(Inject::Key {
                hid: key,
                down: true,
            });
            out.push(Inject::Key {
                hid: key,
                down: false,
            });
        }
    }
}
