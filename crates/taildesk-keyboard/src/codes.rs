//! Conversão entre o HID usage do fio e os códigos de tecla de cada SO.
//!
//! - Linux: código evdev (`KEY_*` de `input-event-codes.h`); no XKB o keycode é evdev + 8.
//! - Windows: scancode do conjunto 1 com o bit `0xE000` para as teclas estendidas, como em
//!   `MapVirtualKeyExW(MAPVK_VSC_TO_VK_EX)` e `KEYEVENTF_EXTENDEDKEY`.

use crate::Hid;

/// `(hid, evdev, scancode)`. A ordem importa: na volta (SO -> HID) vale a primeira linha.
const TABLE: &[(Hid, u16, u16)] = &[
    // Letras a..z
    (0x04, 30, 0x1E),
    (0x05, 48, 0x30),
    (0x06, 46, 0x2E),
    (0x07, 32, 0x20),
    (0x08, 18, 0x12),
    (0x09, 33, 0x21),
    (0x0A, 34, 0x22),
    (0x0B, 35, 0x23),
    (0x0C, 23, 0x17),
    (0x0D, 36, 0x24),
    (0x0E, 37, 0x25),
    (0x0F, 38, 0x26),
    (0x10, 50, 0x32),
    (0x11, 49, 0x31),
    (0x12, 24, 0x18),
    (0x13, 25, 0x19),
    (0x14, 16, 0x10),
    (0x15, 19, 0x13),
    (0x16, 31, 0x1F),
    (0x17, 20, 0x14),
    (0x18, 22, 0x16),
    (0x19, 47, 0x2F),
    (0x1A, 17, 0x11),
    (0x1B, 45, 0x2D),
    (0x1C, 21, 0x15),
    (0x1D, 44, 0x2C),
    // 1..9, 0
    (0x1E, 2, 0x02),
    (0x1F, 3, 0x03),
    (0x20, 4, 0x04),
    (0x21, 5, 0x05),
    (0x22, 6, 0x06),
    (0x23, 7, 0x07),
    (0x24, 8, 0x08),
    (0x25, 9, 0x09),
    (0x26, 10, 0x0A),
    (0x27, 11, 0x0B),
    (0x28, 28, 0x1C), // Enter
    (0x29, 1, 0x01),  // Esc
    (0x2A, 14, 0x0E), // Backspace
    (0x2B, 15, 0x0F), // Tab
    (0x2C, 57, 0x39), // Espaço
    (0x2D, 12, 0x0C), // - _
    (0x2E, 13, 0x0D), // = +
    (0x2F, 26, 0x1A), // [ {  (´ ` no ABNT2)
    (0x30, 27, 0x1B), // ] }  ([ { no ABNT2)
    (0x31, 43, 0x2B), // \ |  (ANSI)
    (0x32, 43, 0x2B), // Non-US # ~ (ISO: ] } no ABNT2), mesma tecla física do 0x31
    (0x33, 39, 0x27), // ; :  (ç no ABNT2)
    (0x34, 40, 0x28), // ' "  (~ ^ no ABNT2)
    (0x35, 41, 0x29), // ` ~  (' " no ABNT2)
    (0x36, 51, 0x33), // ,
    (0x37, 52, 0x34), // .
    (0x38, 53, 0x35), // /    (; : no ABNT2)
    (0x39, 58, 0x3A), // Caps Lock
    (0x3A, 59, 0x3B), // F1
    (0x3B, 60, 0x3C),
    (0x3C, 61, 0x3D),
    (0x3D, 62, 0x3E),
    (0x3E, 63, 0x3F),
    (0x3F, 64, 0x40),
    (0x40, 65, 0x41),
    (0x41, 66, 0x42),
    (0x42, 67, 0x43),
    (0x43, 68, 0x44),    // F10
    (0x44, 87, 0x57),    // F11
    (0x45, 88, 0x58),    // F12
    (0x46, 99, 0xE037),  // PrintScreen
    (0x47, 70, 0x46),    // Scroll Lock
    (0x48, 119, 0xE11D), // Pause
    (0x49, 110, 0xE052), // Insert
    (0x4A, 102, 0xE047), // Home
    (0x4B, 104, 0xE049), // PageUp
    (0x4C, 111, 0xE053), // Delete
    (0x4D, 107, 0xE04F), // End
    (0x4E, 109, 0xE051), // PageDown
    (0x4F, 106, 0xE04D), // Right
    (0x50, 105, 0xE04B), // Left
    (0x51, 108, 0xE050), // Down
    (0x52, 103, 0xE048), // Up
    (0x53, 69, 0xE045),  // Num Lock
    (0x54, 98, 0xE035),  // KP /
    (0x55, 55, 0x37),    // KP *
    (0x56, 74, 0x4A),    // KP -
    (0x57, 78, 0x4E),    // KP +
    (0x58, 96, 0xE01C),  // KP Enter
    (0x59, 79, 0x4F),    // KP 1
    (0x5A, 80, 0x50),
    (0x5B, 81, 0x51),
    (0x5C, 75, 0x4B),
    (0x5D, 76, 0x4C),
    (0x5E, 77, 0x4D),
    (0x5F, 71, 0x47),
    (0x60, 72, 0x48),
    (0x61, 73, 0x49),    // KP 9
    (0x62, 82, 0x52),    // KP 0
    (0x63, 83, 0x53),    // KP . (, no ABNT2)
    (0x64, 86, 0x56),    // Non-US \ | (ISO, \ | no ABNT2)
    (0x65, 127, 0xE05D), // Application (menu)
    (0x67, 117, 0x59),   // KP =
    (0x68, 183, 0x64),   // F13
    (0x69, 184, 0x65),
    (0x6A, 185, 0x66),
    (0x6B, 186, 0x67),
    (0x6C, 187, 0x68),
    (0x6D, 188, 0x69),
    (0x6E, 189, 0x6A),
    (0x6F, 190, 0x6B),
    (0x70, 191, 0x6C),
    (0x71, 192, 0x6D),
    (0x72, 193, 0x6E),
    (0x73, 194, 0x76),   // F24
    (0x85, 121, 0x7E),   // Keypad Comma (. do numérico do ABNT2)
    (0x87, 89, 0x73),    // International1 (/ ? do ABNT2)
    (0x89, 124, 0x7D),   // International3 (iene)
    (0xE0, 29, 0x1D),    // LCtrl
    (0xE1, 42, 0x2A),    // LShift
    (0xE2, 56, 0x38),    // LAlt
    (0xE3, 125, 0xE05B), // LMeta
    (0xE4, 97, 0xE01D),  // RCtrl
    (0xE5, 54, 0x36),    // RShift
    (0xE6, 100, 0xE038), // RAlt / AltGr
    (0xE7, 126, 0xE05C), // RMeta
];

/// HID canônico de uma tecla física: 0x32 (Non-US `#`) e 0x31 (`\\`) são a mesma tecla no
/// Windows (scancode 0x2B) e no Linux (`KEY_BACKSLASH`), então tudo usa 0x31.
pub fn canonical(hid: Hid) -> Hid {
    if hid == 0x32 { 0x31 } else { hid }
}

/// Teclas com código conhecido, na ordem da tabela.
pub fn known_hids() -> impl Iterator<Item = Hid> {
    TABLE.iter().map(|&(h, _, _)| h)
}

pub fn hid_to_evdev(hid: Hid) -> Option<u16> {
    TABLE
        .iter()
        .find(|&&(h, _, _)| h == hid)
        .map(|&(_, e, _)| e)
}

pub fn evdev_to_hid(evdev: u16) -> Option<Hid> {
    TABLE
        .iter()
        .find(|&&(_, e, _)| e == evdev)
        .map(|&(h, _, _)| h)
}

pub fn hid_to_scancode(hid: Hid) -> Option<u16> {
    TABLE
        .iter()
        .find(|&&(h, _, _)| h == hid)
        .map(|&(_, _, s)| s)
}

pub fn scancode_to_hid(scancode: u16) -> Option<Hid> {
    TABLE
        .iter()
        .find(|&&(_, _, s)| s == scancode)
        .map(|&(h, _, _)| h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for hid in known_hids() {
            let back = evdev_to_hid(hid_to_evdev(hid).unwrap()).unwrap();
            // 0x31 e 0x32 são a mesma tecla física: os dois voltam como 0x31.
            assert_eq!(back, if hid == 0x32 { 0x31 } else { hid }, "{hid:#x}");
            let back = scancode_to_hid(hid_to_scancode(hid).unwrap()).unwrap();
            assert_eq!(back, if hid == 0x32 { 0x31 } else { hid }, "{hid:#x}");
        }
    }

    #[test]
    fn well_known_codes() {
        assert_eq!(hid_to_evdev(0x04), Some(30)); // KEY_A
        assert_eq!(hid_to_evdev(0x87), Some(89)); // KEY_RO
        assert_eq!(hid_to_scancode(0x87), Some(0x73)); // ABNT_C1
        assert_eq!(hid_to_scancode(0x85), Some(0x7E)); // ABNT_C2
        assert_eq!(hid_to_scancode(0xE6), Some(0xE038));
    }
}
