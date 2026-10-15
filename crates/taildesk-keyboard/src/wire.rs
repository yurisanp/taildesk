//! Conversão entre [`KeyMsg`]/[`Mode`] e as mensagens de `taildesk_proto::control`.

use taildesk_proto::control::{
    Key, KeyChar, KeyShortcut, KeyboardMode, KeyboardModeKind, Msg, ReleaseAll,
};

use crate::{Hid, KeyMsg, Locks, Mode, Mods};

impl KeyMsg {
    /// Mensagem de controle correspondente.
    pub fn to_proto(&self) -> Msg {
        match self {
            &KeyMsg::Key { hid, down, locks } => Msg::Key(Key {
                hid: hid.into(),
                down,
                locks: locks.map(Locks::bits),
            }),
            &KeyMsg::Shortcut {
                base_char,
                hid_hint,
                mods,
                down,
            } => Msg::KeyShortcut(KeyShortcut {
                base_char: base_char.to_string(),
                hid_hint: hid_hint.into(),
                mods: mods.bits(),
                down,
            }),
            KeyMsg::Char { text, hid_hint } => Msg::KeyChar(KeyChar {
                text: text.clone(),
                hid_hint: (*hid_hint).into(),
            }),
            KeyMsg::ReleaseAll => Msg::ReleaseAll(ReleaseAll {}),
        }
    }

    /// Lê uma mensagem de teclado já validada por `decode_frame`. `None` = não é de teclado
    /// (ou atalho sem caractere, que o host ignora).
    pub fn from_proto(msg: &Msg) -> Option<KeyMsg> {
        let hid = |h: u32| Hid::try_from(h).ok();
        Some(match msg {
            Msg::Key(k) => KeyMsg::Key {
                hid: hid(k.hid)?,
                down: k.down,
                locks: k.locks.map(Locks::from_bits),
            },
            Msg::KeyShortcut(k) => KeyMsg::Shortcut {
                base_char: k.base_char.chars().next()?,
                hid_hint: hid(k.hid_hint)?,
                // AltGr nunca faz parte de um atalho.
                mods: Mods::from_bits(k.mods & 0x0F),
                down: k.down,
            },
            Msg::KeyChar(k) => KeyMsg::Char {
                text: k.text.clone(),
                hid_hint: hid(k.hid_hint)?,
            },
            Msg::ReleaseAll(_) => KeyMsg::ReleaseAll,
            _ => return None,
        })
    }
}

impl Mode {
    pub fn to_proto(self) -> KeyboardMode {
        let mode = match self {
            Mode::Auto => KeyboardModeKind::Auto,
            Mode::Physical => KeyboardModeKind::Physical,
            Mode::Text => KeyboardModeKind::Text,
        };
        KeyboardMode { mode: mode.into() }
    }

    /// Valor desconhecido vira Auto.
    pub fn from_proto(m: &KeyboardMode) -> Mode {
        match KeyboardModeKind::try_from(m.mode) {
            Ok(KeyboardModeKind::Physical) => Mode::Physical,
            Ok(KeyboardModeKind::Text) => Mode::Text,
            _ => Mode::Auto,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taildesk_proto::control::{Control, decode_frame, encode_frame};

    #[test]
    fn round_trip_pelo_fio() {
        let msgs = [
            KeyMsg::Key {
                hid: 0x39,
                down: true,
                locks: Some(Locks {
                    caps: true,
                    num: false,
                }),
            },
            KeyMsg::Key {
                hid: 0xE1,
                down: false,
                locks: None,
            },
            KeyMsg::Shortcut {
                base_char: 'ç',
                hid_hint: 0x33,
                mods: Mods::CTRL.with(Mods::SHIFT),
                down: true,
            },
            KeyMsg::Char {
                text: "ç't".into(),
                hid_hint: 0x06,
            },
            KeyMsg::ReleaseAll,
        ];
        for m in msgs {
            let mut fio = Vec::new();
            encode_frame(&Control::from(m.to_proto()), &mut fio);
            let (lida, _) = decode_frame(&fio).unwrap().unwrap();
            assert_eq!(KeyMsg::from_proto(&lida.msg.unwrap()), Some(m));
        }
        for mode in [Mode::Auto, Mode::Physical, Mode::Text] {
            assert_eq!(Mode::from_proto(&mode.to_proto()), mode);
        }
    }

    #[test]
    fn atalho_vazio_e_ignorado() {
        let m = Msg::KeyShortcut(KeyShortcut {
            base_char: String::new(),
            hid_hint: 4,
            mods: 2,
            down: true,
        });
        assert_eq!(KeyMsg::from_proto(&m), None);
    }
}
