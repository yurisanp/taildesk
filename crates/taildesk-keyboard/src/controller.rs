//! Lado do controlador: eventos de tecla do SO local -> [`KeyMsg`] (`11-teclado.md` §2.3).

use std::collections::HashMap;

use crate::{
    Hid, KeyClass, KeyMsg, Keymap, Level, Locks, Mods, Output, Translation, base_char, classify,
    is_keypad, is_modifier, keypad_nav,
};

/// Como uma tecla apertada foi mandada, para o "soltar" ir pelo mesmo caminho.
#[derive(Clone, Copy, Debug)]
enum Held {
    Physical(Hid),
    Shortcut {
        base_char: char,
        mods: Mods,
    },
    /// Caractere: o host faz apertar+soltar na hora, não há "soltar" a mandar.
    Char,
}

/// Tradutor do controlador. Recebe cada evento de tecla do SO (com o `LCtrl` falso do AltGr do
/// Windows já descartado e convertido em [`Mods::ALTGR`]) e produz as mensagens do fio.
#[derive(Debug)]
pub struct Controller {
    keymap: Keymap,
    translation: Translation,
    locks: Locks,
    /// Tecla morta esperando a próxima tecla.
    pending: Option<char>,
    held: HashMap<Hid, Held>,
}

impl Controller {
    pub fn new(keymap: Keymap, translation: Translation) -> Self {
        Controller {
            keymap,
            translation,
            locks: Locks::default(),
            pending: None,
            held: HashMap::new(),
        }
    }

    pub fn translation(&self) -> Translation {
        self.translation
    }

    /// Troca de layout no meio da sessão: solta o que estiver apertado e descarta a morta pendente.
    pub fn set_keymap(&mut self, keymap: Keymap, out: &mut Vec<KeyMsg>) {
        self.release_all(out);
        self.keymap = keymap;
    }

    /// Troca de modo (ou de host com layout diferente) no meio da sessão.
    pub fn set_translation(&mut self, translation: Translation, out: &mut Vec<KeyMsg>) {
        self.release_all(out);
        self.translation = translation;
    }

    /// Estado real dos locks, lido do SO (na conexão e quando o SO avisar).
    pub fn set_locks(&mut self, locks: Locks) {
        self.locks = locks;
    }

    pub fn locks(&self) -> Locks {
        self.locks
    }

    /// Perda de foco da janela da sessão: o host solta tudo.
    pub fn focus_lost(&mut self, out: &mut Vec<KeyMsg>) {
        self.release_all(out);
    }

    fn release_all(&mut self, out: &mut Vec<KeyMsg>) {
        self.pending = None;
        self.held.clear();
        out.push(KeyMsg::ReleaseAll);
    }

    /// Um evento de tecla. Repetições do SO chegam como novos `down` e viram novas mensagens.
    pub fn key(&mut self, hid: Hid, down: bool, mods: Mods, out: &mut Vec<KeyMsg>) {
        let hid = crate::codes::canonical(hid);
        if !down {
            self.key_up(hid, out);
            return;
        }
        match hid {
            0x39 => self.locks.caps = !self.locks.caps,
            0x53 => self.locks.num = !self.locks.num,
            _ => {}
        }
        match self.translation {
            Translation::Physical => self.physical_down(hid, hid, true, out),
            Translation::Hybrid => self.hybrid_down(hid, mods, out),
        }
    }

    fn key_up(&mut self, hid: Hid, out: &mut Vec<KeyMsg>) {
        let locks = self.physical_locks();
        match self.held.remove(&hid) {
            Some(Held::Physical(sent)) => out.push(KeyMsg::Key {
                hid: sent,
                down: false,
                locks,
            }),
            Some(Held::Shortcut { base_char, mods }) => out.push(KeyMsg::Shortcut {
                base_char,
                hid_hint: hid,
                mods,
                down: false,
            }),
            Some(Held::Char) | None => {}
        }
    }

    fn physical_locks(&self) -> Option<Locks> {
        (self.translation == Translation::Physical).then_some(self.locks)
    }

    fn physical_down(&mut self, hid: Hid, sent: Hid, keep_pending: bool, out: &mut Vec<KeyMsg>) {
        if !keep_pending {
            self.pending = None;
        }
        self.held.insert(hid, Held::Physical(sent));
        out.push(KeyMsg::Key {
            hid: sent,
            down: true,
            locks: self.physical_locks(),
        });
    }

    /// O que a tecla produz no layout local com os modificadores e o Caps Lock atuais.
    fn output(&self, hid: Hid, mods: Mods) -> Option<Output> {
        if is_keypad(hid) && !self.locks.num && keypad_nav(hid).is_some() {
            return None;
        }
        let mut level = Level::from_mods(mods);
        if self.locks.caps && self.keymap.caps_affects(hid) {
            level = level.caps_flipped();
        }
        self.keymap.output(hid, level)
    }

    fn hybrid_down(&mut self, hid: Hid, mods: Mods, out: &mut Vec<KeyMsg>) {
        let output = self.output(hid, mods);
        match classify(hid, mods, output.is_some()) {
            KeyClass::Physical => {
                // Teclado numérico sem Num Lock vira a navegação equivalente: o host pode estar
                // com o Num Lock ligado.
                let sent = if is_keypad(hid) && !self.locks.num {
                    keypad_nav(hid).unwrap_or(hid)
                } else {
                    hid
                };
                self.physical_down(hid, sent, is_modifier(hid), out);
            }
            KeyClass::Shortcut => {
                // O teclado numérico fica no mesmo lugar em todo layout: atalho por posição.
                let letter = (!is_keypad(hid))
                    .then(|| base_char(&self.keymap, hid))
                    .flatten();
                let Some(base_char) = letter else {
                    let sent = if is_keypad(hid) && !self.locks.num {
                        keypad_nav(hid).unwrap_or(hid)
                    } else {
                        hid
                    };
                    self.physical_down(hid, sent, false, out);
                    return;
                };
                self.pending = None;
                let mods = shortcut_mods(mods);
                self.held.insert(hid, Held::Shortcut { base_char, mods });
                out.push(KeyMsg::Shortcut {
                    base_char,
                    hid_hint: hid,
                    mods,
                    down: true,
                });
            }
            KeyClass::Char => {
                self.held.insert(hid, Held::Char);
                if let Some(text) = output.and_then(|o| self.compose(o)) {
                    out.push(KeyMsg::Char {
                        text,
                        hid_hint: hid,
                    });
                }
            }
        }
    }

    /// Resolve teclas mortas como o SO do controlador faz. `None` = morta guardada, nada a mandar.
    fn compose(&mut self, output: Output) -> Option<String> {
        let alone = |km: &Keymap, d: char| km.compose(d, ' ').unwrap_or(d);
        let text = match (self.pending.take(), output) {
            (None, Output::Char(c)) => c.to_string(),
            (None, Output::Dead(d)) => {
                self.pending = Some(d);
                return None;
            }
            (Some(p), Output::Char(c)) => match self.keymap.compose(p, c) {
                Some(r) => r.to_string(),
                None => [alone(&self.keymap, p), c].iter().collect(),
            },
            (Some(p), Output::Dead(d)) => match self.keymap.compose(p, d) {
                Some(r) => r.to_string(),
                None => [alone(&self.keymap, p), alone(&self.keymap, d)]
                    .iter()
                    .collect(),
            },
        };
        Some(text)
    }
}

/// Modificadores que contam num atalho (AltGr nunca chega aqui).
fn shortcut_mods(mods: Mods) -> Mods {
    let mut m = Mods::NONE;
    for bit in [Mods::SHIFT, Mods::CTRL, Mods::ALT, Mods::META] {
        if mods.contains(bit) {
            m = m.with(bit);
        }
    }
    m
}
