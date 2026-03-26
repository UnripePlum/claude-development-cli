use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind, MouseButton};

pub fn encode_key(key: KeyEvent) -> Vec<u8> {
    // Alt modifier sends ESC prefix
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let mut prefix: Vec<u8> = if alt { vec![0x1b] } else { vec![] };

    let encoded = match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii() {
                vec![(c as u8) & 0x1f]
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![0x0d],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                b"\x1b[Z".to_vec()
            } else {
                vec![0x09]
            }
        }
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::F(1) => b"\x1bOP".to_vec(),
        KeyCode::F(2) => b"\x1bOQ".to_vec(),
        KeyCode::F(3) => b"\x1bOR".to_vec(),
        KeyCode::F(4) => b"\x1bOS".to_vec(),
        KeyCode::F(5) => b"\x1b[15~".to_vec(),
        KeyCode::F(6) => b"\x1b[17~".to_vec(),
        KeyCode::F(7) => b"\x1b[18~".to_vec(),
        KeyCode::F(8) => b"\x1b[19~".to_vec(),
        KeyCode::F(9) => b"\x1b[20~".to_vec(),
        KeyCode::F(10) => b"\x1b[21~".to_vec(),
        KeyCode::F(11) => b"\x1b[23~".to_vec(),
        KeyCode::F(12) => b"\x1b[24~".to_vec(),
        _ => vec![],
    };

    if encoded.is_empty() {
        return vec![];
    }

    // For Alt+key, prepend the ESC prefix
    prefix.extend(encoded);
    prefix
}

/// Encode a mouse event into SGR mouse format: \x1b[<button;col;row;M/m
pub fn encode_mouse(mouse: MouseEvent) -> Vec<u8> {
    let (button_code, is_release) = match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => (0u8, false),
        MouseEventKind::Down(MouseButton::Middle) => (1u8, false),
        MouseEventKind::Down(MouseButton::Right) => (2u8, false),
        MouseEventKind::Up(MouseButton::Left) => (0u8, true),
        MouseEventKind::Up(MouseButton::Middle) => (1u8, true),
        MouseEventKind::Up(MouseButton::Right) => (2u8, true),
        MouseEventKind::Drag(MouseButton::Left) => (32u8, false),
        MouseEventKind::Drag(MouseButton::Middle) => (33u8, false),
        MouseEventKind::Drag(MouseButton::Right) => (34u8, false),
        MouseEventKind::Moved => (35u8, false),
        MouseEventKind::ScrollDown => (65u8, false),
        MouseEventKind::ScrollUp => (64u8, false),
        _ => return vec![],
    };

    // Add modifier bits
    let mut btn = button_code;
    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
        btn |= 4;
    }
    if mouse.modifiers.contains(KeyModifiers::ALT) {
        btn |= 8;
    }
    if mouse.modifiers.contains(KeyModifiers::CONTROL) {
        btn |= 16;
    }

    // SGR format: \x1b[<button;col;row;M (press) or m (release)
    // crossterm uses 0-based coords; terminals expect 1-based
    let col = mouse.column + 1;
    let row = mouse.row + 1;
    let suffix = if is_release { b'm' } else { b'M' };

    format!("\x1b[<{};{};{}{}", btn, col, row, suffix as char)
        .into_bytes()
}
