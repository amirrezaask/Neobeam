//! Input encoding: neutral key/mouse representation -> Neovim notation.
//!
//! Kept free of any windowing dependency so the `app` crate maps `winit` events
//! into these neutral types (AGENT_RUST_PORT.md §7).

#[derive(Debug, Clone, Copy, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
    pub shift: bool,
}

impl Mods {
    pub fn any(&self) -> bool {
        self.ctrl || self.alt || self.meta || self.shift
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Escape,
    Backspace,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Space,
    F(u8),
}

impl NamedKey {
    fn name(self) -> String {
        match self {
            NamedKey::Enter => "CR".into(),
            NamedKey::Escape => "Esc".into(),
            NamedKey::Backspace => "BS".into(),
            NamedKey::Tab => "Tab".into(),
            NamedKey::Up => "Up".into(),
            NamedKey::Down => "Down".into(),
            NamedKey::Left => "Left".into(),
            NamedKey::Right => "Right".into(),
            NamedKey::Delete => "Del".into(),
            NamedKey::Home => "Home".into(),
            NamedKey::End => "End".into(),
            NamedKey::PageUp => "PageUp".into(),
            NamedKey::PageDown => "PageDown".into(),
            NamedKey::Insert => "Insert".into(),
            NamedKey::Space => "Space".into(),
            NamedKey::F(n) => format!("F{n}"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum KeyInput {
    Char(char),
    Named(NamedKey),
}

fn mod_prefix(mods: Mods, include_shift: bool) -> String {
    let mut s = String::new();
    if mods.ctrl {
        s.push_str("C-");
    }
    if mods.alt {
        s.push_str("A-");
    }
    if mods.meta {
        s.push_str("D-");
    }
    if include_shift && mods.shift {
        s.push_str("S-");
    }
    s
}

/// Encode a key event into a Neovim `nvim_input` string, or `None` to drop it.
pub fn encode_key(input: &KeyInput, mods: Mods) -> Option<String> {
    match input {
        KeyInput::Named(named) => {
            let prefix = mod_prefix(mods, true);
            Some(format!("<{}{}>", prefix, named.name()))
        }
        KeyInput::Char(c) => {
            let c = *c;
            // Literals that must be escaped regardless of mods.
            let escaped = match c {
                '<' => Some("lt"),
                '\\' => Some("Bslash"),
                _ => None,
            };

            // Shift is not added for printable single chars (the char already
            // arrives shifted), per §7.1.
            let needs_wrap = mods.ctrl || mods.alt || mods.meta || escaped.is_some();
            if !needs_wrap {
                return Some(c.to_string());
            }
            let prefix = mod_prefix(mods, false);
            let key = match escaped {
                Some(name) => name.to_string(),
                None => c.to_string(),
            };
            Some(format!("<{prefix}{key}>"))
        }
    }
}

/// Mouse button + action for `nvim_input_mouse`.
#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Debug, Clone, Copy)]
pub enum MouseAction {
    Press,
    Drag,
    Release,
}

impl MouseButton {
    pub fn nvim_button(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Middle => "middle",
            MouseButton::Right => "right",
            _ => "wheel",
        }
    }
}

impl MouseAction {
    pub fn nvim_action(self) -> &'static str {
        match self {
            MouseAction::Press => "press",
            MouseAction::Drag => "drag",
            MouseAction::Release => "release",
        }
    }
}

/// Action string for a wheel button.
pub fn wheel_action(button: MouseButton) -> Option<&'static str> {
    match button {
        MouseButton::WheelUp => Some("up"),
        MouseButton::WheelDown => Some("down"),
        MouseButton::WheelLeft => Some("left"),
        MouseButton::WheelRight => Some("right"),
        _ => None,
    }
}

pub fn mods_string(mods: Mods) -> String {
    let mut s = String::new();
    if mods.ctrl {
        s.push('C');
    }
    if mods.shift {
        s.push('S');
    }
    if mods.alt {
        s.push('A');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_char() {
        assert_eq!(encode_key(&KeyInput::Char('a'), Mods::default()).unwrap(), "a");
    }

    #[test]
    fn lt_escape() {
        assert_eq!(encode_key(&KeyInput::Char('<'), Mods::default()).unwrap(), "<lt>");
    }

    #[test]
    fn bslash_escape() {
        assert_eq!(encode_key(&KeyInput::Char('\\'), Mods::default()).unwrap(), "<Bslash>");
    }

    #[test]
    fn ctrl_char() {
        let m = Mods { ctrl: true, ..Default::default() };
        assert_eq!(encode_key(&KeyInput::Char('a'), m).unwrap(), "<C-a>");
    }

    #[test]
    fn named_with_shift() {
        let m = Mods { shift: true, ..Default::default() };
        assert_eq!(encode_key(&KeyInput::Named(NamedKey::Tab), m).unwrap(), "<S-Tab>");
    }

    #[test]
    fn fkey() {
        assert_eq!(encode_key(&KeyInput::Named(NamedKey::F(5)), Mods::default()).unwrap(), "<F5>");
    }
}
