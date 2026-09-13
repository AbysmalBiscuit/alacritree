//! Where egui key presses meet the bindings table.  `bindings` names keys and
//! modifiers in its own types so the config parser links no GUI framework, and
//! this module translates at the input boundary.

use strum::IntoEnumIterator;

use crate::bindings::{self, BindingAction, Key, KeyBinding, Modifiers};

/// Every binding that fires for an egui key press.  A key no binding can name
/// fires nothing.
pub fn matches(
    bindings: &[KeyBinding],
    key: egui::Key,
    mods: egui::Modifiers,
) -> Vec<&BindingAction> {
    match Key::iter().find(|k| egui_key(*k) == key) {
        Some(key) => bindings::all_matches(bindings, key, binding_mods(mods)),
        None => Vec::new(),
    }
}

/// How the palette spells a binding's trigger.
pub fn format(key: Key, mods: Modifiers) -> String {
    let mods = egui::Modifiers {
        alt: mods.alt,
        ctrl: mods.ctrl,
        shift: mods.shift,
        mac_cmd: false,
        command: mods.command,
    };
    egui::KeyboardShortcut::new(mods, egui_key(key))
        .format(&egui::ModifierNames::NAMES, cfg!(target_os = "macos"))
}

/// egui-winit raises `command` alongside `ctrl` on every Ctrl press off macOS,
/// and sets `mac_cmd` only where `command` already says the same, so dropping
/// `mac_cmd` loses nothing `Modifiers::fires` needs.
fn binding_mods(mods: egui::Modifiers) -> Modifiers {
    Modifiers { alt: mods.alt, ctrl: mods.ctrl, shift: mods.shift, command: mods.command }
}

fn egui_key(key: Key) -> egui::Key {
    match key {
        Key::ArrowDown => egui::Key::ArrowDown,
        Key::ArrowLeft => egui::Key::ArrowLeft,
        Key::ArrowRight => egui::Key::ArrowRight,
        Key::ArrowUp => egui::Key::ArrowUp,
        Key::Escape => egui::Key::Escape,
        Key::Tab => egui::Key::Tab,
        Key::Backspace => egui::Key::Backspace,
        Key::Enter => egui::Key::Enter,
        Key::Space => egui::Key::Space,
        Key::Insert => egui::Key::Insert,
        Key::Delete => egui::Key::Delete,
        Key::Home => egui::Key::Home,
        Key::End => egui::Key::End,
        Key::PageUp => egui::Key::PageUp,
        Key::PageDown => egui::Key::PageDown,
        Key::Colon => egui::Key::Colon,
        Key::Comma => egui::Key::Comma,
        Key::Backslash => egui::Key::Backslash,
        Key::Slash => egui::Key::Slash,
        Key::OpenBracket => egui::Key::OpenBracket,
        Key::CloseBracket => egui::Key::CloseBracket,
        Key::Backtick => egui::Key::Backtick,
        Key::Minus => egui::Key::Minus,
        Key::Period => egui::Key::Period,
        Key::Plus => egui::Key::Plus,
        Key::Equals => egui::Key::Equals,
        Key::Semicolon => egui::Key::Semicolon,
        Key::Quote => egui::Key::Quote,
        Key::Num0 => egui::Key::Num0,
        Key::Num1 => egui::Key::Num1,
        Key::Num2 => egui::Key::Num2,
        Key::Num3 => egui::Key::Num3,
        Key::Num4 => egui::Key::Num4,
        Key::Num5 => egui::Key::Num5,
        Key::Num6 => egui::Key::Num6,
        Key::Num7 => egui::Key::Num7,
        Key::Num8 => egui::Key::Num8,
        Key::Num9 => egui::Key::Num9,
        Key::A => egui::Key::A,
        Key::B => egui::Key::B,
        Key::C => egui::Key::C,
        Key::D => egui::Key::D,
        Key::E => egui::Key::E,
        Key::F => egui::Key::F,
        Key::G => egui::Key::G,
        Key::H => egui::Key::H,
        Key::I => egui::Key::I,
        Key::J => egui::Key::J,
        Key::K => egui::Key::K,
        Key::L => egui::Key::L,
        Key::M => egui::Key::M,
        Key::N => egui::Key::N,
        Key::O => egui::Key::O,
        Key::P => egui::Key::P,
        Key::Q => egui::Key::Q,
        Key::R => egui::Key::R,
        Key::S => egui::Key::S,
        Key::T => egui::Key::T,
        Key::U => egui::Key::U,
        Key::V => egui::Key::V,
        Key::W => egui::Key::W,
        Key::X => egui::Key::X,
        Key::Y => egui::Key::Y,
        Key::Z => egui::Key::Z,
        Key::F1 => egui::Key::F1,
        Key::F2 => egui::Key::F2,
        Key::F3 => egui::Key::F3,
        Key::F4 => egui::Key::F4,
        Key::F5 => egui::Key::F5,
        Key::F6 => egui::Key::F6,
        Key::F7 => egui::Key::F7,
        Key::F8 => egui::Key::F8,
        Key::F9 => egui::Key::F9,
        Key::F10 => egui::Key::F10,
        Key::F11 => egui::Key::F11,
        Key::F12 => egui::Key::F12,
        Key::F13 => egui::Key::F13,
        Key::F14 => egui::Key::F14,
        Key::F15 => egui::Key::F15,
        Key::F16 => egui::Key::F16,
        Key::F17 => egui::Key::F17,
        Key::F18 => egui::Key::F18,
        Key::F19 => egui::Key::F19,
        Key::F20 => egui::Key::F20,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{NamedAction, RawBinding, parse_bindings};

    fn named(bindings: &[KeyBinding], key: egui::Key, mods: egui::Modifiers) -> Vec<NamedAction> {
        matches(bindings, key, mods)
            .into_iter()
            .filter_map(|a| match a {
                BindingAction::Named(n) => Some(*n),
                _ => None,
            })
            .collect()
    }

    /// A key mapped onto another key's egui counterpart would fire that key's
    /// bindings instead of its own.
    #[test]
    fn every_binding_key_has_its_own_egui_key() {
        for key in Key::iter() {
            assert_eq!(Key::iter().find(|k| egui_key(*k) == egui_key(key)), Some(key));
        }
    }

    /// egui-winit's Ctrl press carries `command` too off macOS; a Ctrl binding
    /// still fires on it, and an unmodified binding on the same key does not.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn a_ctrl_press_carrying_command_fires_ctrl_bindings_only() {
        let bindings = parse_bindings(vec![RawBinding {
            key: "L".into(),
            mods: None,
            mode: None,
            chars: None,
            action: Some("ToggleSessionRows".into()),
            command: None,
        }]);
        let ctrl = egui::Modifiers { ctrl: true, command: true, ..egui::Modifiers::NONE };
        assert_eq!(named(&bindings, egui::Key::K, ctrl), vec![NamedAction::TogglePalette]);
        assert!(named(&bindings, egui::Key::L, ctrl).is_empty());
        assert_eq!(named(&bindings, egui::Key::L, egui::Modifiers::NONE), vec![
            NamedAction::ToggleSessionRows
        ]);
    }

    #[test]
    fn a_key_no_binding_names_fires_nothing() {
        let bindings = parse_bindings(Vec::new());
        assert!(matches(&bindings, egui::Key::F35, egui::Modifiers::NONE).is_empty());
    }
}
