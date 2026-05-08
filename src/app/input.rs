use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, NamedKey};

pub fn encode_key(event: &KeyEvent) -> Option<Vec<u8>> {
    if event.state != ElementState::Pressed {
        return None;
    }
    match &event.logical_key {
        Key::Named(NamedKey::Enter) => Some(b"\r".to_vec()),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
        Key::Named(NamedKey::Tab) => Some(b"\t".to_vec()),
        Key::Named(NamedKey::Escape) => Some(b"\x1b".to_vec()),
        _ => event.text.as_ref().map(|s| s.as_bytes().to_vec()),
    }
}
