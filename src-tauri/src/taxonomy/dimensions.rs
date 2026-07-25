//! Dimension math from ASSETS.md §2. Everything the pipeline ships is **author px**,
//! and the invariant is `author = GU * 2`.

/// Author px = game units × 2 (ASSETS.md "Units convention").
pub const fn author_px(gu: u32) -> u32 {
    gu * 2
}

/// Cell size in game units for a grid shape (ASSETS.md §2 Step 1 table). Falls back to
/// a size that fits the board into the central safe area for non-standard grids.
pub fn cell_gu(cols: u32, rows: u32) -> u32 {
    match (cols, rows) {
        (5, 3) => 140,
        (5, 4) => 130,
        (6, 5) => 110,
        (7, 7) => 90,
        _ => fallback_cell_gu(cols, rows),
    }
}

/// Fit the board into ~1120 × 620 GU (central area with HUD/background headroom),
/// rounded down to the nearest 10 GU to keep numbers tidy.
fn fallback_cell_gu(cols: u32, rows: u32) -> u32 {
    let by_w = 1120 / cols.max(1);
    let by_h = 620 / rows.max(1);
    let cell = by_w.min(by_h);
    (cell / 10).max(1) * 10
}

/// Full-body mascot character (ASSETS.md §10 feature hero): portrait, beside the reels.
pub const MASCOT_AUTHOR: (u32, u32) = (1200, 1800);

/// Standard symbol author size: fills a 1×1 cell.
pub fn symbol_author(cols: u32, rows: u32) -> (u32, u32) {
    let a = author_px(cell_gu(cols, rows));
    (a, a)
}

/// Oversize symbol (wild/scatter) author size: 1.30 × 1.15 of a cell (§2 Step 5).
/// Currently unused — the studio ships wild/scatter square — but kept for the spec option.
#[allow(dead_code)]
pub fn symbol_oversize_author(cols: u32, rows: u32) -> (u32, u32) {
    let cell = cell_gu(cols, rows) as f32;
    let w = author_px((cell * 1.30).round() as u32);
    let h = author_px((cell * 1.15).round() as u32);
    (w, h)
}

/// Win-highlight glow author size: 1.5 × cell (§5, allows bleed).
pub fn win_highlight_author(cols: u32, rows: u32) -> (u32, u32) {
    let cell = cell_gu(cols, rows) as f32;
    let a = author_px((cell * 1.5).round() as u32);
    (a, a)
}

/// Full-canvas author sizes per orientation (ASSETS.md §1/§3).
pub const CANVAS_LANDSCAPE: (u32, u32) = (3200, 1800);
pub const CANVAS_PORTRAIT: (u32, u32) = (1800, 3200);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_is_double_gu() {
        assert_eq!(author_px(140), 280);
    }

    #[test]
    fn standard_grids_match_spec_table() {
        assert_eq!(cell_gu(5, 3), 140);
        assert_eq!(cell_gu(5, 4), 130);
        assert_eq!(cell_gu(6, 5), 110);
        assert_eq!(cell_gu(7, 7), 90);
    }

    #[test]
    fn symbol_sizes_match_spec_examples() {
        // 5x3 lines: cell 140 GU -> 280 author px symbol (§2 Step 2).
        assert_eq!(symbol_author(5, 3), (280, 280));
        // 5x3 wild/scatter -> 364 x 322 author px (§2 Step 5).
        assert_eq!(symbol_oversize_author(5, 3), (364, 322));
        // 5x3 win highlight -> 420 x 420 author px (§5).
        assert_eq!(win_highlight_author(5, 3), (420, 420));
        // 6x5 scatter: cell 110 GU -> 220 author px symbol.
        assert_eq!(symbol_author(6, 5), (220, 220));
    }
}
