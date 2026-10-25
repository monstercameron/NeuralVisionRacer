//! Direct fills for the large, simple shapes (dial faces, rims, panels). tiny-skia's general anti-aliased
//! rasteriser costs several nanoseconds per covered pixel, which made the big flat areas most of a frame; these
//! compute coverage from a signed distance instead and blend constant runs in a tight loop.
//!
//! All coordinates are in pixels. Blending is premultiplied source-over, the same as tiny-skia's default.

use tiny_skia::Pixmap;

use crate::design::Rgba;

/// A disc of radius `r`.
pub fn disc(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, color: Rgba) {
    let bbox = (cx - r, cy - r, cx + r, cy + r);
    fill(pm, bbox, color, |px, py| ((px - cx).hypot(py - cy)) - r, |py| (chord(cx, cy, r - 0.75, py), None));
}

/// A ring centred on radius `r`, `width` wide.
pub fn ring(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, width: f32, color: Rgba) {
    let outer = r + width / 2.0;
    let bbox = (cx - outer, cy - outer, cx + outer, cy + outer);
    let hole = r - width / 2.0 - 0.75;
    fill(pm, bbox, color, |px, py| ((px - cx).hypot(py - cy) - r).abs() - width / 2.0, |py| (None, chord(cx, cy, hole, py)));
}

/// The x range of a circle's chord at row `py`, if the row crosses it.
fn chord(cx: f32, cy: f32, r: f32, py: f32) -> Option<(f32, f32)> {
    let dy = (py - cy).abs();
    (r > 0.0 && dy < r).then(|| {
        let half = (r * r - dy * dy).sqrt();
        (cx - half, cx + half)
    })
}

/// A rectangle with corner radius `radius`.
pub fn rounded_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgba) {
    let rad = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    let (cx, cy, hw, hh) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
    let sdf = |px: f32, py: f32| {
        let qx = (px - cx).abs() - (hw - rad);
        let qy = (py - cy).abs() - (hh - rad);
        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad
    };
    fill(pm, (x, y, x + w, y + h), color, sdf, |py| {
        // Rows clear of the corners are solid between the straight sides.
        (((py - cy).abs() < hh - rad.max(0.75)).then(|| (x + 0.75, x + w - 0.75)), None)
    });
}

/// Fills the pixels in `bbox` with coverage `0.5 − sdf(pixel centre)`. `spans(row centre y)` may return an x range
/// where coverage is certainly full (filled without evaluating the distance) and one where it is certainly zero
/// (skipped).
fn fill(
    pm: &mut Pixmap,
    bbox: (f32, f32, f32, f32),
    color: Rgba,
    sdf: impl Fn(f32, f32) -> f32,
    spans: impl Fn(f32) -> (Option<(f32, f32)>, Option<(f32, f32)>),
) {
    if !color.visible() {
        return;
    }
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let x0 = (bbox.0 - 1.0).floor().max(0.0) as i32;
    let y0 = (bbox.1 - 1.0).floor().max(0.0) as i32;
    let x1 = ((bbox.2 + 1.0).ceil() as i32).min(w);
    let y1 = ((bbox.3 + 1.0).ceil() as i32).min(h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let [r, g, b, a] = color.0;
    // Premultiplied source in 0..=255 and its alpha as a 0..=1 float, for coverage scaling.
    let pre = |c: u8| (c as u32 * a as u32 + 127) / 255;
    let src = [pre(r), pre(g), pre(b), a as u32];
    let data = pm.data_mut();
    let stride = w as usize * 4;
    for j in y0..y1 {
        let py = j as f32 + 0.5;
        let (solid, hole) = spans(py);
        let inside = |(s0, s1): (f32, f32)| ((s0 - 0.5).ceil() as i32, (s1 - 0.5).floor() as i32);
        let (span, hole) = (solid.map(inside), hole.map(inside));
        let row = &mut data[j as usize * stride..(j as usize + 1) * stride];
        let mut i = x0;
        while i < x1 {
            if let Some((s0, s1)) = span {
                if i >= s0 && i <= s1 {
                    let end = (s1 + 1).min(x1);
                    for px in row[i as usize * 4..end as usize * 4].chunks_exact_mut(4) {
                        blend(px, src, 256);
                    }
                    i = end;
                    continue;
                }
            }
            if let Some((h0, h1)) = hole {
                if i >= h0 && i <= h1 {
                    i = h1 + 1;
                    continue;
                }
            }
            let coverage = (0.5 - sdf(i as f32 + 0.5, py)).clamp(0.0, 1.0);
            if coverage > 0.0 {
                let k = (coverage * 256.0) as u32;
                let px = &mut row[i as usize * 4..i as usize * 4 + 4];
                blend(px, src, k);
            }
            i += 1;
        }
    }
}

/// Premultiplied source-over with the source scaled by `k / 256`.
#[inline(always)]
fn blend(dst: &mut [u8], src: [u32; 4], k: u32) {
    let sa = (src[3] * k) >> 8;
    let inv = 255 - sa;
    for c in 0..4 {
        let s = if c == 3 { sa } else { (src[c] * k) >> 8 };
        dst[c] = (s + div255(dst[c] as u32 * inv)).min(255) as u8;
    }
}

/// `x / 255`, rounded, for `x` up to 255 × 255.
#[inline(always)]
fn div255(x: u32) -> u32 {
    let t = x + 128;
    (t + (t >> 8)) >> 8
}
