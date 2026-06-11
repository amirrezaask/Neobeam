//! Box-drawing characters rendered as filled rectangles for crisp joins
//! (AGENT_RUST_PORT.md §5.3, §13.9). Double-line variants fall back to glyphs.

#[derive(Clone, Copy, Default)]
struct Arms {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

fn arms_for(ch: char) -> Option<Arms> {
    let a = |left, right, up, down| {
        Some(Arms {
            left,
            right,
            up,
            down,
        })
    };
    match ch {
        '─' => a(true, true, false, false),
        '│' => a(false, false, true, true),
        '┌' | '╭' => a(false, true, false, true),
        '┐' | '╮' => a(true, false, false, true),
        '└' | '╰' => a(false, true, true, false),
        '┘' | '╯' => a(true, false, true, false),
        '├' => a(false, true, true, true),
        '┤' => a(true, false, true, true),
        '┬' => a(true, true, false, true),
        '┴' => a(true, true, true, false),
        '┼' => a(true, true, true, true),
        _ => None,
    }
}

pub fn is_box_glyph(ch: char) -> bool {
    arms_for(ch).is_some()
}

/// Emit arm rectangles `(x, y, w, h)` for a box-drawing char within a cell.
pub fn box_rects(
    ch: char,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    thickness: f32,
) -> Vec<(f32, f32, f32, f32)> {
    let Some(arms) = arms_for(ch) else {
        return Vec::new();
    };
    let t = thickness.max(1.0);
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    let half = t * 0.5;
    let mut out = Vec::new();
    if arms.left {
        out.push((x, cy - half, cx - x + half, t));
    }
    if arms.right {
        out.push((cx - half, cy - half, x + w - cx + half, t));
    }
    if arms.up {
        out.push((cx - half, y, t, cy - y + half));
    }
    if arms.down {
        out.push((cx - half, cy - half, t, y + h - cy + half));
    }
    out
}
