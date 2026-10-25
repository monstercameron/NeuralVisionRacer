//! A tiny built-in vector font, so the crate needs no font files or font library.
//!
//! Two faces:
//! - a stroked face: each glyph is a few polylines on a 4 × 6 grid (cap height 6), drawn with round caps and joins.
//!   Curves are chamfered, which reads like the DIN-style lettering on instrument clusters and stays legible at
//!   small sizes. Upper-case letters, digits, a few lower-case letters (`k m h x i n` for "km/h" and "x1000/min";
//!   other lower-case letters fall back to capitals) and `- + . , : / ° % ( )` and space.
//! - a seven-segment face for digital readouts: digits, `-`, `.`, space, and `r n` / `R N` for a gear readout.
//!
//! Both build paths in the caller's units: `size` is the cap (digit) height, `x` the left edge, `y` the top.

use tiny_skia::PathBuilder;

/// Gap between stroked glyphs, in grid units.
const GAP: f32 = 1.7;

/// (advance width in grid units, polylines). Polylines are separated by `;`, points by spaces, `x,y`.
fn glyph(c: char) -> Option<(f32, &'static str)> {
    let g = match c {
        '0' | 'O' => (4.0, "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0"),
        '1' => (4.0, "0.8,1.4 2.4,0 2.4,6"),
        '2' => (4.0, "0,1 1,0 3,0 4,1 4,2.2 0,6 4,6"),
        '3' => (4.0, "0,1 1,0 3,0 4,1 4,2 3,2.9 4,3.8 4,5 3,6 1,6 0,5; 1.4,2.9 3,2.9"),
        '4' => (4.0, "3,6 3,0 0,4.2 4,4.2"),
        '5' => (4.0, "4,0 0.4,0 0.2,2.7 3,2.7 4,3.7 4,5 3,6 1,6 0,5"),
        '6' => (4.0, "3.6,0 1,0 0,1 0,5 1,6 3,6 4,5 4,3.7 3,2.7 0,2.7"),
        '7' => (4.0, "0,0 4,0 4,1 1.4,6"),
        '8' => (4.0, "1,0 3,0 4,1 4,1.9 3,2.8 1,2.8 0,1.9 0,1 1,0; 1,2.8 3,2.8 4,3.8 4,5 3,6 1,6 0,5 0,3.8 1,2.8"),
        '9' => (4.0, "0.4,6 3,6 4,5 4,1 3,0 1,0 0,1 0,2.3 1,3.3 4,3.3"),
        'A' => (4.0, "0,6 0,1.5 1.5,0 2.5,0 4,1.5 4,6; 0,3.7 4,3.7"),
        'B' => (4.0, "0,0 3,0 4,1 4,1.9 3,2.8 0,2.8; 3,2.8 4,3.8 4,5 3,6 0,6 0,0"),
        'C' => (4.0, "4,1 3,0 1,0 0,1 0,5 1,6 3,6 4,5"),
        'D' => (4.0, "0,0 2.8,0 4,1.2 4,4.8 2.8,6 0,6 0,0"),
        'E' => (4.0, "4,0 0,0 0,6 4,6; 0,2.9 3,2.9"),
        'F' => (4.0, "4,0 0,0 0,6; 0,2.9 3,2.9"),
        'G' => (4.0, "4,1 3,0 1,0 0,1 0,5 1,6 3,6 4,5 4,3.2 2.2,3.2"),
        'H' => (4.0, "0,0 0,6; 4,0 4,6; 0,2.9 4,2.9"),
        'I' => (2.0, "1,0 1,6"),
        'J' => (4.0, "4,0 4,5 3,6 1,6 0,5"),
        'K' => (4.0, "0,0 0,6; 4,0 0.4,3.4; 1.6,2.3 4,6"),
        'L' => (4.0, "0,0 0,6 4,6"),
        'M' => (4.6, "0,6 0,0 2.3,3.4 4.6,0 4.6,6"),
        'N' => (4.0, "0,6 0,0 4,6 4,0"),
        'P' => (4.0, "0,6 0,0 3,0 4,1 4,2.2 3,3.2 0,3.2"),
        'Q' => (4.0, "1,0 3,0 4,1 4,5 3,6 1,6 0,5 0,1 1,0; 2.4,4.4 4,6"),
        'R' => (4.0, "0,6 0,0 3,0 4,1 4,2.2 3,3.2 0,3.2; 2,3.2 4,6"),
        'S' => (4.0, "4,1 3,0 1,0 0,1 0,2 1,2.9 3,2.9 4,3.9 4,5 3,6 1,6 0,5"),
        'T' => (4.0, "0,0 4,0; 2,0 2,6"),
        'U' => (4.0, "0,0 0,5 1,6 3,6 4,5 4,0"),
        'V' => (4.0, "0,0 2,6 4,0"),
        'W' => (4.6, "0,0 1.1,6 2.3,2.4 3.5,6 4.6,0"),
        'X' => (4.0, "0,0 4,6; 4,0 0,6"),
        'Y' => (4.0, "0,0 2,3 4,0; 2,3 2,6"),
        'Z' => (4.0, "0,0 4,0 0,6 4,6"),
        'k' => (3.4, "0,0 0,6; 3.2,2.2 0,4.4; 1.1,3.6 3.4,6"),
        'm' => (4.4, "0,6 0,2.2; 0,3 0.7,2.2 1.5,2.2 2.2,3 2.2,6; 2.2,3 2.9,2.2 3.7,2.2 4.4,3 4.4,6"),
        'h' => (3.4, "0,0 0,6; 0,3.2 0.9,2.2 2.5,2.2 3.4,3.1 3.4,6"),
        'n' => (3.4, "0,6 0,2.2; 0,3.2 0.9,2.2 2.5,2.2 3.4,3.1 3.4,6"),
        'i' => (1.0, "0.5,2.4 0.5,6; 0.5,0.5 0.5,0.7"),
        'x' => (3.4, "0,2.2 3.4,6; 3.4,2.2 0,6"),
        '-' => (3.0, "0.2,3.2 2.8,3.2"),
        '+' => (3.6, "0,3 3.6,3; 1.8,1.2 1.8,4.8"),
        '.' => (0.6, "0.3,5.8 0.3,6"),
        ',' => (0.8, "0.6,5.6 0.6,6 0.1,6.8"),
        ':' => (0.6, "0.3,1.8 0.3,2.0; 0.3,5.6 0.3,5.8"),
        '/' => (3.0, "0,6.2 3,-0.2"),
        '°' => (2.0, "0.6,0 1.4,0 2,0.6 2,1.4 1.4,2 0.6,2 0,1.4 0,0.6 0.6,0"),
        '%' => (4.0, "0,6 4,0; 0.5,0.5 1.2,0.5 1.2,1.6 0.5,1.6 0.5,0.5; 2.8,4.4 3.5,4.4 3.5,5.5 2.8,5.5 2.8,4.4"),
        '(' => (1.6, "1.6,-0.4 0.4,1 0.4,5 1.6,6.4"),
        ')' => (1.6, "0,-0.4 1.2,1 1.2,5 0,6.4"),
        ' ' => (2.6, ""),
        _ => return None,
    };
    Some(g)
}

fn resolve(c: char) -> Option<(f32, &'static str)> {
    glyph(c).or_else(|| glyph(c.to_ascii_uppercase()))
}

/// Width of `text` in the stroked face at cap height `size`, without the trailing gap.
pub fn stroke_width(text: &str, size: f32) -> f32 {
    let unit = size / 6.0;
    let mut w = 0.0;
    let mut any = false;
    for c in text.chars() {
        if let Some((advance, _)) = resolve(c) {
            w += advance + GAP;
            any = true;
        }
    }
    if any {
        w -= GAP;
    }
    w * unit
}

/// Adds `text` in the stroked face, left edge `x`, top `y`, cap height `size`, to `pb` (to be stroked).
pub fn stroke_text(pb: &mut PathBuilder, text: &str, x: f32, y: f32, size: f32) {
    let unit = size / 6.0;
    let mut pen = x;
    for c in text.chars() {
        let Some((advance, lines)) = resolve(c) else { continue };
        for line in lines.split(';') {
            let mut first = true;
            for point in line.split_whitespace() {
                let Some((px, py)) = point.split_once(',') else { continue };
                let (Ok(px), Ok(py)) = (px.parse::<f32>(), py.parse::<f32>()) else { continue };
                let (px, py) = (pen + px * unit, y + py * unit);
                if first {
                    pb.move_to(px, py);
                    first = false;
                } else {
                    pb.line_to(px, py);
                }
            }
        }
        pen += (advance + GAP) * unit;
    }
}

// Seven-segment face. A digit cell is SEG_W × 1 (times the size), cells are SEG_ADVANCE apart.
const SEG_W: f32 = 0.56;
const SEG_ADVANCE: f32 = 0.72;
const DOT_ADVANCE: f32 = 0.24;
const SLANT: f32 = 0.08;

/// Segment bits a..g (bit 0 = a, top; then b top-right, c bottom-right, d bottom, e bottom-left, f top-left, g middle).
fn segments(c: char) -> Option<u8> {
    Some(match c {
        '0' | 'O' => 0b0111111,
        '1' => 0b0000110,
        '2' => 0b1011011,
        '3' => 0b1001111,
        '4' => 0b1100110,
        '5' => 0b1101101,
        '6' => 0b1111101,
        '7' => 0b0000111,
        '8' => 0b1111111,
        '9' => 0b1101111,
        '-' => 0b1000000,
        'r' | 'R' => 0b1010000,
        'n' | 'N' => 0b1010100,
        ' ' => 0,
        _ => return None,
    })
}

/// Width of `text` in the seven-segment face at digit height `size`.
pub fn segment_width(text: &str, size: f32) -> f32 {
    let mut w = 0.0;
    let mut last_cell = false;
    for c in text.chars() {
        if c == '.' {
            w += DOT_ADVANCE;
            last_cell = false;
        } else if segments(c).is_some() {
            w += SEG_ADVANCE;
            last_cell = true;
        }
    }
    if last_cell {
        w -= SEG_ADVANCE - SEG_W;
    }
    w * size
}

/// Adds `text` as filled seven-segment polygons, left edge `x`, top `y`, digit height `size`, to `pb`.
/// With `all_segments`, every cell draws all seven segments (the unlit "ghost" pattern behind a readout).
pub fn segment_text(pb: &mut PathBuilder, text: &str, x: f32, y: f32, size: f32, weight: f32, all_segments: bool) {
    let h = size;
    let w = SEG_W * size;
    let t = (weight * size).max(0.5);
    let gap = t * 0.12;
    let mut pen = x;
    for c in text.chars() {
        if c == '.' {
            let s = t * 0.95;
            let dx = pen + DOT_ADVANCE * size * 0.5 - s * 0.5;
            if !all_segments {
                poly(pb, &[(dx, y + h - s), (dx + s, y + h - s), (dx + s, y + h), (dx, y + h)], y, h);
            }
            pen += DOT_ADVANCE * size;
            continue;
        }
        let Some(bits) = segments(c) else { continue };
        let bits = if all_segments { 0x7f } else { bits };
        let (l, r) = (pen + t / 2.0, pen + w - t / 2.0);
        let (top, mid, bot) = (y + t / 2.0, y + h / 2.0, y + h - t / 2.0);
        let horizontal = |pb: &mut PathBuilder, yy: f32| {
            let (a, b) = (l + gap, r - gap);
            poly(pb, &[(a, yy), (a + t / 2.0, yy - t / 2.0), (b - t / 2.0, yy - t / 2.0), (b, yy), (b - t / 2.0, yy + t / 2.0), (a + t / 2.0, yy + t / 2.0)], y, h);
        };
        let vertical = |pb: &mut PathBuilder, xx: f32, y0: f32, y1: f32| {
            let (a, b) = (y0 + gap, y1 - gap);
            poly(pb, &[(xx, a), (xx + t / 2.0, a + t / 2.0), (xx + t / 2.0, b - t / 2.0), (xx, b), (xx - t / 2.0, b - t / 2.0), (xx - t / 2.0, a + t / 2.0)], y, h);
        };
        if bits & 1 != 0 {
            horizontal(pb, top);
        }
        if bits & 2 != 0 {
            vertical(pb, r, top, mid);
        }
        if bits & 4 != 0 {
            vertical(pb, r, mid, bot);
        }
        if bits & 8 != 0 {
            horizontal(pb, bot);
        }
        if bits & 16 != 0 {
            vertical(pb, l, mid, bot);
        }
        if bits & 32 != 0 {
            vertical(pb, l, top, mid);
        }
        if bits & 64 != 0 {
            horizontal(pb, mid);
        }
        pen += SEG_ADVANCE * size;
    }
}

/// A closed polygon, slanted forwards (italic) about the cell's baseline.
fn poly(pb: &mut PathBuilder, points: &[(f32, f32)], top: f32, h: f32) {
    for (i, &(px, py)) in points.iter().enumerate() {
        let px = px + (top + h - py) * SLANT;
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    pb.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_parses() {
        for c in "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZkmhnix-+.,:/°%() ".chars() {
            let (_, lines) = glyph(c).unwrap_or_else(|| panic!("glyph {c:?}"));
            for point in lines.split(';').flat_map(str::split_whitespace) {
                let (x, y) = point.split_once(',').unwrap_or_else(|| panic!("{c:?}: {point}"));
                assert!(x.parse::<f32>().is_ok() && y.parse::<f32>().is_ok(), "{c:?}: {point}");
            }
        }
    }

    #[test]
    fn widths_grow_with_text() {
        assert!(stroke_width("RPM", 10.0) > stroke_width("RP", 10.0));
        assert_eq!(segment_width("88", 10.0), (SEG_ADVANCE + SEG_W) * 10.0);
    }
}
