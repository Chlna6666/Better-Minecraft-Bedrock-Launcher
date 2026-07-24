use crate::platform::winit::cursor_style_to_icon;
use crate::*;

pub(crate) fn mouse_button_from_winit(button: winit::event::MouseButton) -> Option<MouseButton> {
    match button {
        winit::event::MouseButton::Left => Some(MouseButton::Left),
        winit::event::MouseButton::Right => Some(MouseButton::Right),
        winit::event::MouseButton::Middle => Some(MouseButton::Middle),
        winit::event::MouseButton::Back => Some(MouseButton::Navigate(NavigationDirection::Back)),
        winit::event::MouseButton::Forward => {
            Some(MouseButton::Navigate(NavigationDirection::Forward))
        }
        winit::event::MouseButton::Other(_) => None,
    }
}

pub(crate) fn modifiers_from_winit(modifiers: winit::keyboard::ModifiersState) -> Modifiers {
    Modifiers {
        control: modifiers.control_key(),
        alt: modifiers.alt_key(),
        shift: modifiers.shift_key(),
        platform: modifiers.super_key(),
        function: false,
    }
}

pub(crate) fn keystroke_from_winit(
    logical_key: &winit::keyboard::Key,
    modifiers: Modifiers,
    text: &Option<winit::keyboard::SmolStr>,
) -> Option<Keystroke> {
    use winit::keyboard::Key as WinitKey;
    use winit::keyboard::NamedKey;

    let (key, key_char) = match logical_key {
        WinitKey::Named(named) => {
            let key_name = match named {
                NamedKey::Backspace => "backspace",
                NamedKey::Tab => "tab",
                NamedKey::Enter => "enter",
                NamedKey::Escape => "escape",
                NamedKey::Space => "space",
                NamedKey::ArrowLeft => "left",
                NamedKey::ArrowRight => "right",
                NamedKey::ArrowUp => "up",
                NamedKey::ArrowDown => "down",
                NamedKey::Home => "home",
                NamedKey::End => "end",
                NamedKey::PageUp => "pageup",
                NamedKey::PageDown => "pagedown",
                NamedKey::Insert => "insert",
                NamedKey::Delete => "delete",
                NamedKey::F1 => "f1",
                NamedKey::F2 => "f2",
                NamedKey::F3 => "f3",
                NamedKey::F4 => "f4",
                NamedKey::F5 => "f5",
                NamedKey::F6 => "f6",
                NamedKey::F7 => "f7",
                NamedKey::F8 => "f8",
                NamedKey::F9 => "f9",
                NamedKey::F10 => "f10",
                NamedKey::F11 => "f11",
                NamedKey::F12 => "f12",
                NamedKey::Shift
                | NamedKey::Control
                | NamedKey::Alt
                | NamedKey::Super
                | NamedKey::Meta => return None,
                _ => return None,
            };
            let key_char = match named {
                NamedKey::Space
                    if !modifiers.control
                        && !modifiers.platform
                        && !modifiers.function
                        && !modifiers.alt =>
                {
                    Some(" ".to_string())
                }
                _ => None,
            };
            (key_name.to_string(), key_char)
        }
        WinitKey::Character(ch) => {
            let key = ch.to_lowercase();
            let key_char = text.as_ref().map(|value| value.to_string()).or_else(|| {
                if !modifiers.control
                    && !modifiers.platform
                    && !modifiers.function
                    && !modifiers.alt
                {
                    if modifiers.shift {
                        Some(ch.to_uppercase())
                    } else {
                        Some(ch.to_string())
                    }
                } else {
                    None
                }
            });
            (key, key_char)
        }
        WinitKey::Unidentified(_) | WinitKey::Dead(_) => return None,
    };

    Some(Keystroke {
        modifiers,
        key,
        key_char,
    })
}

pub(crate) fn apply_cursor_style_to_window(window: &winit::window::Window, style: CursorStyle) {
    match style {
        CursorStyle::None => window.set_cursor_visible(false),
        _ => {
            window.set_cursor_visible(true);
            let cursor = cursor_style_to_icon(style)
                .map(winit::window::Cursor::from)
                .unwrap_or(winit::window::Cursor::default());
            window.set_cursor(cursor);
        }
    }
}
