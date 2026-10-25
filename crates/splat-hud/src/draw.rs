//! Drawing the gauges with tiny-skia. Every path is built in design units and scaled to pixels by one transform.

use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::design::*;
use crate::{fast, font};
use crate::{Overlay, Telemetry};

/// Where the panel lands on a screen: pixel rectangle and pixels per design unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

pub fn place(design: &HudDesign, screen_width: u32, screen_height: u32) -> Placement {
    let (sw, sh) = (screen_width as f32, screen_height as f32);
    let margin = (design.margin * sh).min(sw * 0.25).min(sh * 0.25);
    let mut scale = sh * design.height / design.size[1];
    // On a narrow (portrait) screen, shrink to fit the width instead of spilling off the sides.
    let room = (sw - 2.0 * margin).max(1.0);
    if design.size[0] * scale > room {
        scale = room / design.size[0];
    }
    let room_y = (sh - 2.0 * margin).max(1.0);
    if design.size[1] * scale > room_y {
        scale = room_y / design.size[1];
    }
    let width = ((design.size[0] * scale).ceil() as u32).min(screen_width);
    let height = ((design.size[1] * scale).ceil() as u32).min(screen_height);
    let free_x = screen_width.saturating_sub(width) as f32;
    let free_y = screen_height.saturating_sub(height) as f32;
    let (hx, vy) = match design.anchor {
        Anchor::TopLeft => (0, 0),
        Anchor::TopCenter => (1, 0),
        Anchor::TopRight => (2, 0),
        Anchor::CenterLeft => (0, 1),
        Anchor::Center => (1, 1),
        Anchor::CenterRight => (2, 1),
        Anchor::BottomLeft => (0, 2),
        Anchor::BottomCenter => (1, 2),
        Anchor::BottomRight => (2, 2),
    };
    let along = |free: f32, which: i32| match which {
        0 => margin.min(free),
        1 => free / 2.0,
        _ => (free - margin).max(0.0),
    };
    Placement { x: along(free_x, hx).round() as u32, y: along(free_y, vy).round() as u32, width, height, scale }
}

pub fn render(design: &HudDesign, t: &Telemetry, screen_width: u32, screen_height: u32, time_s: f32) -> Overlay {
    let p = place(design, screen_width, screen_height);
    let Some(pixmap) = Pixmap::new(p.width, p.height) else {
        return Overlay { x: p.x, y: p.y, width: 0, height: 0, rgba: Vec::new() };
    };
    let mut c = Canvas { pixmap, base: Transform::from_scale(p.scale, p.scale), scale: p.scale };

    if design.background.visible() {
        let s = p.scale;
        fast::rounded_rect(&mut c.pixmap, 0.0, 0.0, design.size[0] * s, design.size[1] * s, design.corner_radius * s, design.background);
    }
    for gauge in &design.gauges {
        match &gauge.kind {
            GaugeKind::Panel(p) => panel(&mut c, p),
            GaugeKind::Dial(d) => dial_body(&mut c, d, t),
            GaugeKind::Bar(b) => bar(&mut c, b, t),
            GaugeKind::Digital(d) => digital(&mut c, d, t),
            GaugeKind::Gear(g) => gear(&mut c, g, t),
            GaugeKind::ShiftLights(s) => shift_lights(&mut c, s, t, time_s),
            GaugeKind::Text(x) => text_gauge(&mut c, x, t),
        }
    }
    // Needles last, so they pass over readouts printed on (or placed over) the dial faces, as on a real cluster.
    for gauge in &design.gauges {
        if let GaugeKind::Dial(d) = &gauge.kind {
            needle(&mut c, d, t);
        }
    }
    Overlay { x: p.x, y: p.y, width: p.width, height: p.height, rgba: c.pixmap.take() }
}

struct Canvas {
    pixmap: Pixmap,
    base: Transform,
    /// Pixels per design unit.
    scale: f32,
}

fn paint(color: Rgba) -> Paint<'static> {
    let [r, g, b, a] = color.0;
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

impl Canvas {
    fn fill(&mut self, pb: PathBuilder, color: Rgba) {
        if !color.visible() {
            return;
        }
        if let Some(path) = pb.finish() {
            self.pixmap.fill_path(&path, &paint(color), FillRule::Winding, self.base, None);
        }
    }

    fn fill_path(&mut self, path: &Path, color: Rgba, transform: Transform) {
        if color.visible() {
            self.pixmap.fill_path(path, &paint(color), FillRule::Winding, self.base.pre_concat(transform), None);
        }
    }

    fn stroke(&mut self, pb: PathBuilder, color: Rgba, width: f32, cap: LineCap) {
        if !color.visible() || width <= 0.0 {
            return;
        }
        if let Some(path) = pb.finish() {
            let stroke = Stroke { width, line_cap: cap, line_join: LineJoin::Round, ..Stroke::default() };
            self.pixmap.stroke_path(&path, &paint(color), &stroke, self.base, None);
        }
    }

    /// Text in the stroked face, vertically centred on `y`.
    fn text(&mut self, text: &str, x: f32, y: f32, size: f32, align: Align, color: Rgba, weight: f32) {
        if size <= 0.0 || !color.visible() {
            return;
        }
        let w = font::stroke_width(text, size);
        let left = match align {
            Align::Left => x,
            Align::Center => x - w / 2.0,
            Align::Right => x - w,
        };
        let mut pb = PathBuilder::new();
        font::stroke_text(&mut pb, text, left, y - size / 2.0, size);
        self.stroke(pb, color, size * weight, LineCap::Round);
    }
}

/// A point on a circle, `angle` in degrees clockwise from 12 o'clock.
fn polar(cx: f32, cy: f32, r: f32, angle: f32) -> (f32, f32) {
    let a = angle.to_radians();
    (cx + r * a.sin(), cy - r * a.cos())
}

/// Appends an arc (degrees clockwise from 12 o'clock) as cubic Béziers, starting a new contour.
fn arc(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
    let sweep = a1 - a0;
    let pieces = ((sweep.abs() / 60.0).ceil() as usize).max(1);
    let step = sweep / pieces as f32;
    let k = 4.0 / 3.0 * (step.to_radians() / 4.0).tan() * r;
    let (sx, sy) = polar(cx, cy, r, a0);
    pb.move_to(sx, sy);
    for i in 0..pieces {
        let (b0, b1) = (a0 + step * i as f32, a0 + step * (i + 1) as f32);
        let (x0, y0) = polar(cx, cy, r, b0);
        let (x1, y1) = polar(cx, cy, r, b1);
        // Tangent direction for clockwise travel at angle b: (cos b, sin b).
        let (t0x, t0y) = (b0.to_radians().cos(), b0.to_radians().sin());
        let (t1x, t1y) = (b1.to_radians().cos(), b1.to_radians().sin());
        pb.cubic_to(x0 + k * t0x, y0 + k * t0y, x1 - k * t1x, y1 - k * t1y, x1, y1);
    }
}

fn rounded_rect(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.0 {
        pb.move_to(x, y);
        pb.line_to(x + w, y);
        pb.line_to(x + w, y + h);
        pb.line_to(x, y + h);
        pb.close();
        return;
    }
    let k = 0.5523 * r;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
}

/// A zone's value range this frame, clamped to `min..max`; `None` when it is empty or unresolved.
fn zone_range(zone: &Option<Zone>, min: f32, max: f32, t: &Telemetry) -> Option<(f32, f32, Rgba)> {
    let zone = zone.as_ref()?;
    let from = zone.from.resolve(t)?.max(min);
    let to = match zone.to {
        Some(to) => to.resolve(t)?,
        None => max,
    }
    .min(max);
    (to > from).then_some((from, to, zone.color))
}

fn format_number(value: f32, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, value);
    if s.starts_with('-') && s[1..].chars().all(|c| c == '0' || c == '.') {
        s[1..].to_string()
    } else {
        s
    }
}

fn dial_body(c: &mut Canvas, d: &Dial, t: &Telemetry) {
    let [cx, cy] = d.center;
    let r = d.radius;

    // Face. A gradient would add depth but costs a third of the frame (tiny-skia runs gradients in its slow
    // high-precision pipeline), so the face is flat with a soft shadow band just inside the rim instead.
    let s = c.scale;
    fast::disc(&mut c.pixmap, cx * s, cy * s, r * s, d.face_color);
    if d.face_color.visible() {
        let band = r * 0.06;
        fast::ring(&mut c.pixmap, cx * s, cy * s, (r - d.rim_width - band / 2.0) * s, band * s, Rgba([0, 0, 0, 70]));
    }
    if d.rim_width > 0.0 {
        fast::ring(&mut c.pixmap, cx * s, cy * s, (r - d.rim_width / 2.0) * s, d.rim_width * s, d.rim_color);
    }

    let outer = r - d.rim_width - r * 0.025;
    let value = d.channel.read(t);

    // The value sweep, an arc filling from min to the value.
    // The value sweep: a band under the ticks filling from `sweep_from` to the value (backwards for boost below 0).
    let (a0, a1) = (d.angle_deg(d.sweep_from), d.angle_deg(value));
    if d.sweep_color.visible() && (a1 - a0).abs() > 0.05 {
        let mut pb = PathBuilder::new();
        arc(&mut pb, cx, cy, outer - d.sweep_width * 0.5, a0, a1);
        c.stroke(pb, d.sweep_color, d.sweep_width, LineCap::Butt);
    }

    // Warning and red zones: bands along the outer edge of the scale.
    let red = zone_range(&d.red, d.min, d.max, t);
    for (from, to, color) in [zone_range(&d.warn, d.min, d.max, t), red].into_iter().flatten() {
        if d.zone_width > 0.0 {
            let mut pb = PathBuilder::new();
            arc(&mut pb, cx, cy, outer - d.zone_width / 2.0, d.angle_deg(from), d.angle_deg(to));
            c.stroke(pb, color, d.zone_width, LineCap::Butt);
        }
    }
    let in_red = |v: f32| red.is_some_and(|(from, to, _)| v >= from - 1e-3 && v <= to + 1e-3);
    let red_color = red.map_or(d.tick_color, |(_, _, c)| c);

    // Ticks: majors every `major`, minors every `minor` in between, both counted from min.
    let span = d.max - d.min;
    let eps = span * 1e-4;
    let mut ticks = [PathBuilder::new(), PathBuilder::new(), PathBuilder::new(), PathBuilder::new()]; // major, minor, red major, red minor
    let step = if d.minor > 0.0 { d.minor } else { d.major };
    let count = ((span + eps) / step).floor() as usize;
    for i in 0..=count.min(2000) {
        let v = d.min + step * i as f32;
        let k = (v - d.min) / d.major;
        let major = (k - k.round()).abs() * d.major <= eps.max(step * 1e-3);
        let len = if major { d.tick_length } else { d.tick_length * 0.5 };
        let a = d.angle_deg(v);
        let (x0, y0) = polar(cx, cy, outer, a);
        let (x1, y1) = polar(cx, cy, outer - len, a);
        let which = (!major) as usize + 2 * in_red(v) as usize;
        ticks[which].move_to(x0, y0);
        ticks[which].line_to(x1, y1);
    }
    let [major, minor, red_major, red_minor] = ticks;
    c.stroke(major, d.tick_color, d.tick_width, LineCap::Butt);
    c.stroke(minor, d.tick_color, d.tick_width * 0.6, LineCap::Butt);
    c.stroke(red_major, red_color, d.tick_width, LineCap::Butt);
    c.stroke(red_minor, red_color, d.tick_width * 0.6, LineCap::Butt);

    // Numbers.
    if d.label_size > 0.0 && d.label_color.visible() {
        let mut normal = PathBuilder::new();
        let mut redp = PathBuilder::new();
        let count = ((d.max + eps - d.labels_from) / d.label_every).floor();
        if count >= 0.0 {
            for i in 0..=(count as usize).min(500) {
                let v = d.labels_from + d.label_every * i as f32;
                let label = format_number(v * d.label_scale, d.label_decimals);
                let (lx, ly) = polar(cx, cy, r * d.label_radius, d.angle_deg(v));
                let w = font::stroke_width(&label, d.label_size);
                let pb = if in_red(v) { &mut redp } else { &mut normal };
                font::stroke_text(pb, &label, lx - w / 2.0, ly - d.label_size / 2.0, d.label_size);
            }
        }
        let width = d.label_size * 0.13;
        c.stroke(normal, d.label_color, width, LineCap::Round);
        c.stroke(redp, red_color, width, LineCap::Round);
    }
}

fn needle(c: &mut Canvas, d: &Dial, t: &Telemetry) {
    let [cx, cy] = d.center;
    let r = d.radius;
    if d.needle_color.visible() {
        let (len, tail, w) = (r * d.needle_length, r * 0.16, d.needle_width);
        let mut pb = PathBuilder::new();
        pb.move_to(cx - w * 0.6, cy + tail);
        pb.line_to(cx - w * 0.5, cy);
        pb.line_to(cx - w * 0.16, cy - len);
        pb.line_to(cx + w * 0.16, cy - len);
        pb.line_to(cx + w * 0.5, cy);
        pb.line_to(cx + w * 0.6, cy + tail);
        pb.close();
        if let Some(path) = pb.finish() {
            let angle = d.angle_deg(d.channel.read(t));
            let rotate = Transform::from_rotate_at(angle, cx, cy);
            let shadow = Transform::from_translate(r * 0.015, r * 0.03).pre_concat(rotate);
            c.fill_path(&path, Rgba([0, 0, 0, 110]), shadow);
            c.fill_path(&path, d.needle_color, rotate);
        }
    }
    if d.hub_radius > 0.0 {
        let s = c.scale;
        fast::disc(&mut c.pixmap, cx * s, cy * s, d.hub_radius * s, d.hub_color);
        fast::ring(&mut c.pixmap, cx * s, cy * s, d.hub_radius * 0.92 * s, d.hub_radius * 0.16 * s, d.hub_color.lightened(0.25));
    }
}

fn bar_color(b: &Bar, v: f32, t: &Telemetry) -> Rgba {
    if let Some((from, _, color)) = zone_range(&b.red, b.min, b.max, t) {
        if v >= from {
            return color;
        }
    }
    if let Some((from, _, color)) = zone_range(&b.warn, b.min, b.max, t) {
        if v >= from {
            return color;
        }
    }
    if b.low_below.is_some_and(|low| v < low) {
        return b.low_color;
    }
    b.color
}

fn bar(c: &mut Canvas, b: &Bar, t: &Telemetry) {
    let [cx, cy] = b.center;
    let [w, h] = b.size;
    let (x0, y0) = (cx - w / 2.0, cy - h / 2.0);
    let vertical = b.vertical();
    let value = b.channel.read(t);
    let frac = ((value - b.min) / (b.max - b.min)).clamp(0.0, 1.0);
    let length = if vertical { h } else { w };
    let thick = if vertical { w } else { h };
    // A sub-rectangle of the bar from fraction f0 to f1 along its length.
    let span = |f0: f32, f1: f32| -> (f32, f32, f32, f32) {
        if vertical {
            (x0, y0 + h * (1.0 - f1), w, h * (f1 - f0))
        } else {
            (x0 + w * f0, y0, w * (f1 - f0), h)
        }
    };
    let radius = thick * 0.2;

    if b.segments > 0 {
        let n = b.segments as f32;
        let gap = (length / n) * 0.22;
        let mut lit_by_color: Vec<(Rgba, PathBuilder)> = Vec::new();
        let mut off = PathBuilder::new();
        for i in 0..b.segments {
            let (f0, f1) = (i as f32 / n, (i + 1) as f32 / n);
            let g = gap / length / 2.0;
            let (x, y, sw, sh) = span(f0 + g, f1 - g);
            let lit = frac >= f0 + 0.5 / n || (i == 0 && frac > 0.0 && value > b.min);
            if lit {
                // A cell takes the colour of the value at its far end, so zones show where they start.
                let color = bar_color(b, b.min + (b.max - b.min) * f1.min(frac.max(f0)), t);
                let pb = match lit_by_color.iter_mut().find(|(c, _)| *c == color) {
                    Some((_, pb)) => pb,
                    None => {
                        lit_by_color.push((color, PathBuilder::new()));
                        &mut lit_by_color.last_mut().unwrap().1
                    }
                };
                rounded_rect(pb, x, y, sw, sh, radius * 0.5);
            } else {
                rounded_rect(&mut off, x, y, sw, sh, radius * 0.5);
            }
        }
        c.fill(off, b.track_color);
        for (color, pb) in lit_by_color {
            c.fill(pb, color);
        }
    } else {
        let mut track = PathBuilder::new();
        rounded_rect(&mut track, x0, y0, w, h, radius);
        c.fill(track, b.track_color);
        // Tint the track where the warning and red zones lie.
        for (from, to, color) in [zone_range(&b.warn, b.min, b.max, t), zone_range(&b.red, b.min, b.max, t)].into_iter().flatten() {
            let f = |v: f32| (v - b.min) / (b.max - b.min);
            let (x, y, sw, sh) = span(f(from), f(to));
            let mut pb = PathBuilder::new();
            rounded_rect(&mut pb, x, y, sw, sh, radius);
            c.fill(pb, color.faded(0.35));
        }
        if frac > 0.0 {
            let (x, y, sw, sh) = span(0.0, frac);
            let mut pb = PathBuilder::new();
            rounded_rect(&mut pb, x, y, sw, sh, radius);
            c.fill(pb, bar_color(b, value, t));
        }
    }

    // Horizontal: label above the left end, value above the right end. Vertical: label below, value above.
    let gap = thick * 0.7;
    let line_size = b.label_size.max(if b.show_value { b.value_size } else { 0.0 });
    let above = y0 - gap - line_size / 2.0;
    if !b.label.is_empty() {
        if vertical {
            c.text(&b.label, cx, y0 + h + gap + b.label_size / 2.0, b.label_size, Align::Center, b.label_color, 0.12);
        } else {
            c.text(&b.label, x0, above, b.label_size, Align::Left, b.label_color, 0.12);
        }
    }
    if b.show_value {
        let text = format!("{}{}", format_number(value, b.decimals), b.units);
        let color = bar_color(b, value, t);
        let color = if color == b.color { b.value_color } else { color };
        if vertical {
            c.text(&text, cx, y0 - gap - b.value_size / 2.0, b.value_size, Align::Center, color, 0.13);
        } else {
            c.text(&text, x0 + w, above, b.value_size, Align::Right, color, 0.13);
        }
    }
}

fn panel(c: &mut Canvas, p: &Panel) {
    let [cx, cy] = p.center;
    let [w, h] = p.size;
    let s = c.scale;
    fast::rounded_rect(&mut c.pixmap, (cx - w / 2.0) * s, (cy - h / 2.0) * s, w * s, h * s, p.radius * s, p.color);
    if p.border_width > 0.0 {
        let bw = p.border_width;
        let mut pb = PathBuilder::new();
        rounded_rect(&mut pb, cx - w / 2.0 + bw / 2.0, cy - h / 2.0 + bw / 2.0, w - bw, h - bw, (p.radius - bw / 2.0).max(0.0));
        c.stroke(pb, p.border_color, bw, LineCap::Butt);
    }
}

fn digital(c: &mut Canvas, d: &Digital, t: &Telemetry) {
    let value = d.channel.read(t);
    let text = format_number(value, d.decimals);
    let cells = text.chars().filter(|ch| *ch != '.').count();
    let padded = format!("{}{}", " ".repeat(d.digits.saturating_sub(cells)), text);
    let [cx, cy] = d.center;
    match d.style {
        DigitStyle::Segment => {
            let w = font::segment_width(&padded, d.size);
            let left = match d.align {
                Align::Left => cx,
                Align::Center => cx - w / 2.0,
                Align::Right => cx - w,
            };
            let top = cy - d.size / 2.0;
            if d.ghost_color.visible() {
                let mut ghost = PathBuilder::new();
                font::segment_text(&mut ghost, &padded, left, top, d.size, d.weight, true);
                c.fill(ghost, d.ghost_color);
            }
            let mut pb = PathBuilder::new();
            font::segment_text(&mut pb, &padded, left, top, d.size, d.weight, false);
            c.fill(pb, d.color);
        }
        DigitStyle::Stroke => c.text(&text, cx, cy, d.size, d.align, d.color, d.weight),
    }
}

fn gear(c: &mut Canvas, g: &Gear, t: &Telemetry) {
    let [cx, cy] = g.center;
    if g.background.visible() && g.box_size[0] > 0.0 && g.box_size[1] > 0.0 {
        let [w, h] = g.box_size;
        let mut pb = PathBuilder::new();
        rounded_rect(&mut pb, cx - w / 2.0, cy - h / 2.0, w, h, w.min(h) * 0.15);
        c.fill(pb, g.background);
    }
    let (text, color) = match t.gear {
        g_ if g_ < 0 => ("R".to_string(), g.reverse_color),
        0 => ("N".to_string(), g.neutral_color),
        n => (n.to_string(), g.color),
    };
    match g.style {
        DigitStyle::Stroke => c.text(&text, cx, cy, g.size, Align::Center, color, g.weight),
        DigitStyle::Segment => {
            let w = font::segment_width(&text, g.size);
            let mut pb = PathBuilder::new();
            font::segment_text(&mut pb, &text, cx - w / 2.0, cy - g.size / 2.0, g.size, g.weight, false);
            c.fill(pb, color);
        }
    }
}

fn shift_lights(c: &mut Canvas, s: &ShiftLights, t: &Telemetry, time_s: f32) {
    let [cx, cy] = s.center;
    let [w, h] = s.size;
    let n = s.count;
    let pitch = w / n as f32;
    let flashing = s.flashing(t);
    let flash_on = flashing && (time_s * s.flash_hz * 2.0).floor() as i64 % 2 == 0;
    let lit = if flashing { 0 } else { s.lit(t.rpm, t) };

    let mut off = PathBuilder::new();
    let mut groups: Vec<(Rgba, PathBuilder, PathBuilder)> = Vec::new(); // colour, light, glow
    for i in 0..n {
        let x = cx - w / 2.0 + pitch * (i as f32 + 0.5);
        let color = if flashing {
            flash_on.then_some(s.flash_color)
        } else {
            (i < lit).then(|| s.color_of(i))
        };
        let add = |pb: &mut PathBuilder, grow: f32| match s.shape {
            LightShape::Round => pb.push_circle(x, cy, (pitch * 0.38).min(h / 2.0) * grow),
            LightShape::Bar => {
                let (lw, lh) = (pitch * 0.8 * grow, h * grow);
                rounded_rect(pb, x - lw / 2.0, cy - lh / 2.0, lw, lh, lw.min(lh) * 0.25)
            }
        };
        match color {
            None => add(&mut off, 1.0),
            Some(color) => {
                let k = match groups.iter().position(|(c, _, _)| *c == color) {
                    Some(k) => k,
                    None => {
                        groups.push((color, PathBuilder::new(), PathBuilder::new()));
                        groups.len() - 1
                    }
                };
                add(&mut groups[k].1, 1.0);
                add(&mut groups[k].2, 1.55);
            }
        }
    }
    c.fill(off, s.off_color);
    for (color, light, glow) in groups {
        c.fill(glow, color.faded(0.3));
        c.fill(light, color);
    }
}

fn text_gauge(c: &mut Canvas, x: &Text, t: &Telemetry) {
    let color = if x.when.holds(t) { x.color } else { x.off_color };
    c.text(&x.text, x.center[0], x.center[1], x.size, x.align, color, x.weight);
}
