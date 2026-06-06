//! Nerd Font codepoint range detection.
//!
//! Used to gate fallback to the bundled Symbols Nerd Font Mono so we only
//! substitute icon glyphs, not arbitrary missing characters.

/// Returns true if `ch` falls within a standard Nerd Font icon codepoint range.
pub fn is_nerd_glyph(ch: char) -> bool {
    let c = ch as u32;
    matches!(
        c,
        0x23FB..=0x23FE // IEC power symbols
            | 0x2500..=0x259F // box drawing (patched)
            | 0x2630 // powerline extra
            | 0x2665
            | 0x26A1 // octicons
            | 0x276C..=0x2771 // heavy angle brackets
            | 0x2B58 // IEC power
            | 0xE000..=0xE00A // pomicons
            | 0xE0A0..=0xE0A2 // powerline
            | 0xE0A3 // powerline extra
            | 0xE0B0..=0xE0B3 // powerline
            | 0xE0B4..=0xE0C8 // powerline extra
            | 0xE0CA // powerline extra
            | 0xE0CC..=0xE0D7 // powerline extra
            | 0xE200..=0xE2A9 // font awesome extension
            | 0xE300..=0xE3E3 // weather icons
            | 0xE5FA..=0xE6B7 // seti-ui + custom
            | 0xE700..=0xE8EF // devicons
            | 0xEA60..=0xEC1E // codicons
            | 0xED00..=0xEFCE // font awesome (includes progress at EE00..EE0B)
            | 0xF000..=0xF2FF // font awesome
            | 0xF300..=0xF381 // font logos
            | 0xF400..=0xF533 // octicons
            | 0xF500..=0xFD46 // material design
            | 0xF0001..=0xF1AF0 // material design (supplementary plane)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powerline_separators_are_nerd_glyphs() {
        assert!(is_nerd_glyph('\u{E0B0}'));
        assert!(is_nerd_glyph('\u{E0B1}'));
    }

    #[test]
    fn devicons_are_nerd_glyphs() {
        assert!(is_nerd_glyph('\u{E7A8}'));
    }

    #[test]
    fn ascii_is_not_nerd_glyph() {
        assert!(!is_nerd_glyph('A'));
        assert!(!is_nerd_glyph('中'));
    }
}
