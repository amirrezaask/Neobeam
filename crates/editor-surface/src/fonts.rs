//! System font discovery for the settings UI and editor atlas.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use fontdue::{Font, FontSettings};

/// Bundled Symbols Nerd Font Mono (icon fallback for UI chrome).
pub const SYMBOLS_NERD_FONT: &[u8] =
    include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

fn system_font_db() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

/// Load the editor font face from the shared system font database.
pub fn load_editor_font(family: Option<&str>) -> Result<Font> {
    let db = system_font_db();
    let query = fontdb::Query {
        families: &[match family {
            Some(name) => fontdb::Family::Name(name),
            None => fontdb::Family::Monospace,
        }],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    };

    let id = db
        .query(&query)
        .or_else(|| {
            db.query(&fontdb::Query {
                families: &[fontdb::Family::Monospace],
                ..query
            })
        })
        .ok_or_else(|| anyhow!("no monospace font found on this system"))?;

    db.with_face_data(id, |data, index| {
        Font::from_bytes(
            data,
            FontSettings {
                collection_index: index,
                ..FontSettings::default()
            },
        )
        .ok()
    })
    .flatten()
    .ok_or_else(|| anyhow!("failed to parse selected font face"))
}

/// Logical cell metrics for grid sizing before the GPU atlas is ready.
pub fn estimate_cell_size(
    font_family: Option<&str>,
    font_size: f32,
    line_height: f32,
    scale: f32,
) -> Result<(f32, f32)> {
    let font = load_editor_font(font_family)?;
    let px = font_size * scale;
    let m = font.metrics('M', px);
    let cell_w = (m.advance_width / scale).ceil().max(1.0);
    let cell_h = (font_size * line_height).ceil().max(1.0);
    Ok((cell_w, cell_h))
}

/// Load a system Unicode fallback font that covers common non-ASCII symbol ranges
/// (dingbats, arrows, etc.) that patched Nerd Fonts don't include.
/// Tries a prioritized list; returns the first one that loads successfully.
pub fn load_unicode_fallback_font() -> Option<Font> {
    let db = system_font_db();
    let candidates = [
        "Menlo",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Courier New",
    ];
    for name in candidates {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        if let Some(id) = db.query(&query) {
            let font = db
                .with_face_data(id, |data, index| {
                    Font::from_bytes(
                        data,
                        FontSettings {
                            collection_index: index,
                            ..FontSettings::default()
                        },
                    )
                    .ok()
                })
                .flatten();
            if font.is_some() {
                return font;
            }
        }
    }
    None
}

/// Monospace font family names installed on this system, sorted alphabetically.
pub fn list_monospace_fonts() -> Vec<String> {
    let db = system_font_db();
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
