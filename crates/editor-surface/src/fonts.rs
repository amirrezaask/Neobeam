//! System font discovery for the settings UI.

use std::collections::BTreeSet;

/// Bundled Symbols Nerd Font Mono (icon fallback for UI chrome).
pub const SYMBOLS_NERD_FONT: &[u8] =
    include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

/// Monospace font family names installed on this system, sorted alphabetically.
pub fn list_monospace_fonts() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut names = BTreeSet::new();
    for face in db.faces() {
        if face.monospaced {
            if let Some((name, _)) = face.families.first() {
                names.insert(name.clone());
            }
        }
    }
    names.into_iter().collect()
}
