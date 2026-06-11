//! Encode key events as PTY byte sequences.
//! Uses a crate-local key enum so terminal-core stays imgui-independent.

/// Minimal key representation forwarded from the windowing layer.
#[derive(Clone, Copy, Debug)]
pub enum TermKey {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Escape,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    F(u8),
}

/// Encode a key + modifier combination into xterm-compatible PTY bytes.
/// Returns `None` if there is nothing to send.
pub fn encode(
    key: TermKey,
    shift: bool,
    ctrl: bool,
    alt: bool,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    let alt_prefix: &[u8] = if alt { b"\x1b" } else { b"" };

    // ASCII control combinations.
    if ctrl {
        if let TermKey::Char(c) = key {
            let c = c.to_ascii_lowercase();
            let byte = match c {
                '@' | ' ' | '2' => Some(0x00),
                'a'..='z' => Some(c as u8 - b'a' + 1),
                '[' | '3' => Some(0x1b),
                '\\' | '4' => Some(0x1c),
                ']' | '5' => Some(0x1d),
                '^' | '6' => Some(0x1e),
                '_' | '7' | '/' => Some(0x1f),
                '?' | '8' => Some(0x7f),
                _ => None,
            };
            if let Some(byte) = byte {
                let mut out = alt_prefix.to_vec();
                out.push(byte);
                return Some(out);
            }
        }
    }

    // Printable character (no ctrl).
    if !ctrl {
        if let TermKey::Char(c) = key {
            if !c.is_control() {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                let mut out = alt_prefix.to_vec();
                out.extend_from_slice(s.as_bytes());
                return Some(out);
            }
        }
    }

    let modifier = 1 + shift as u8 + (alt as u8 * 2) + (ctrl as u8 * 4);
    let modified = modifier != 1;
    let seq = match key {
        TermKey::Enter => "\r".into(),
        TermKey::Backspace => "\x7f".into(),
        TermKey::Escape => "\x1b".into(),
        TermKey::Tab => "\t".into(),
        TermKey::BackTab => "\x1b[Z".into(),
        TermKey::Up => cursor_sequence('A', modifier, app_cursor),
        TermKey::Down => cursor_sequence('B', modifier, app_cursor),
        TermKey::Right => cursor_sequence('C', modifier, app_cursor),
        TermKey::Left => cursor_sequence('D', modifier, app_cursor),
        TermKey::Home => cursor_sequence('H', modifier, app_cursor),
        TermKey::End => cursor_sequence('F', modifier, app_cursor),
        TermKey::Insert => tilde_sequence(2, modifier),
        TermKey::Delete => tilde_sequence(3, modifier),
        TermKey::PageUp => tilde_sequence(5, modifier),
        TermKey::PageDown => tilde_sequence(6, modifier),
        TermKey::F(n @ 1..=4) if modified => {
            format!("\x1b[1;{}{}", modifier, (b'P' + n - 1) as char)
        }
        TermKey::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char),
        TermKey::F(n @ 5..=12) => {
            let number = [15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize];
            tilde_sequence(number, modifier)
        }
        TermKey::Char(_) | TermKey::F(_) => return None,
    };

    // Alt is encoded in CSI modifier sequences, but prefixes simple keys.
    let mut out = if modified
        && matches!(
            key,
            TermKey::Up
                | TermKey::Down
                | TermKey::Left
                | TermKey::Right
                | TermKey::Home
                | TermKey::End
                | TermKey::Insert
                | TermKey::Delete
                | TermKey::PageUp
                | TermKey::PageDown
                | TermKey::F(_)
        ) {
        Vec::new()
    } else {
        alt_prefix.to_vec()
    };
    out.extend_from_slice(seq.as_bytes());
    Some(out)
}

fn cursor_sequence(final_byte: char, modifier: u8, app_cursor: bool) -> String {
    if modifier != 1 {
        format!("\x1b[1;{modifier}{final_byte}")
    } else if app_cursor {
        format!("\x1bO{final_byte}")
    } else {
        format!("\x1b[{final_byte}")
    }
}

fn tilde_sequence(number: u8, modifier: u8) -> String {
    if modifier == 1 {
        format!("\x1b[{number}~")
    } else {
        format!("\x1b[{number};{modifier}~")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_printable_and_unicode_characters() {
        assert_eq!(
            encode(TermKey::Char('a'), false, false, false, false),
            Some(b"a".to_vec())
        );
        assert_eq!(
            encode(TermKey::Char('λ'), false, false, false, false),
            Some("λ".as_bytes().to_vec())
        );
        assert_eq!(
            encode(TermKey::Char(' '), false, false, false, false),
            Some(b" ".to_vec())
        );
    }

    #[test]
    fn encodes_ctrl_and_alt_characters() {
        assert_eq!(
            encode(TermKey::Char('c'), false, true, false, false),
            Some(vec![3])
        );
        assert_eq!(
            encode(TermKey::Char('x'), false, false, true, false),
            Some(b"\x1bx".to_vec())
        );
        assert_eq!(
            encode(TermKey::Char(' '), false, true, false, false),
            Some(vec![0])
        );
        assert_eq!(
            encode(TermKey::Char('['), false, true, false, false),
            Some(vec![0x1b])
        );
    }

    #[test]
    fn encodes_navigation_and_shift_tab() {
        assert_eq!(
            encode(TermKey::Up, false, false, false, false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode(TermKey::Up, false, false, false, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode(TermKey::Left, false, true, false, false),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(
            encode(TermKey::BackTab, true, false, false, false),
            Some(b"\x1b[Z".to_vec())
        );
    }
}
