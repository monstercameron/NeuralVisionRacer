//! Dashboard gauges for Splat Racer: speedometer, rev counter, boost, temperatures, gear and shift lights, designed
//! per car in a `.hud` text file and drawn into a small premultiplied RGBA image that the game composites over its
//! 3D frame (blend "one, one-minus-src-alpha").
//!
//! Pure Rust (tiny-skia for anti-aliased paths, a built-in vector font for text), so it builds for
//! `wasm32-unknown-unknown`. [`render`] keeps no state: it draws exactly the values it is given, so smoothing, if
//! any, belongs to the caller.
//!
//! ```
//! let design = splat_hud::HudDesign::porsche_992_gt3rs();
//! let telemetry = splat_hud::Telemetry { rpm: 6500.0, redline_rpm: 9000.0, limiter_rpm: 9000.0, gear: 3, ..Default::default() };
//! let overlay = splat_hud::render(&design, &telemetry, 1280, 720);
//! assert_eq!(overlay.rgba.len(), (overlay.width * overlay.height * 4) as usize);
//! ```
//!
//! # The design format
//!
//! The same style as the car spec files: `[section]` headers, `key = value` lines, `#` comments. A comment is a
//! `#` at the start of a line, or a `#` followed by a space (so `color = #FF3A1E  # orange` works: a `#` directly
//! followed by a hex digit is a colour). Unknown sections, unknown keys and keys set twice are errors that name the
//! line, the section and the key. Gauges are drawn in file order; dial needles are drawn after everything else.
//!
//! ## `[hud]`
//! | key | meaning |
//! |---|---|
//! | `name` | display name (required) |
//! | `anchor` | `top-left`, `top-center`, `top-right`, `center-left`, `center`, `center-right`, `bottom-left`, `bottom-center`, `bottom-right` (required) |
//! | `size = w h` | the panel in design units (required). Gauge positions are measured from its top-left corner, y down. |
//! | `height` | the panel's height as a share of the screen height, e.g. `0.34` (required). The panel looks the same at any resolution; on a screen too narrow for it, it shrinks to fit the width. |
//! | `margin` | gap to the anchored screen edges, as a share of the screen height (default `0.02`) |
//! | `background`, `corner_radius` | an optional panel backing (default transparent) |
//!
//! ## `[gauge.<name>]`
//! Every gauge has `type` and `center = x y` (design units). Colours are `#RRGGBB` or `#RRGGBBAA` (straight alpha).
//! `channel` names a [`Telemetry`] field: `speed_kmh` (read as its magnitude), `rpm`, `boost_bar`, `water_temp_c`,
//! `oil_temp_c`, `oil_pressure_bar`, `throttle`, `brake`, `gear`.
//!
//! rpm thresholds (`*_from`, `*_to`, shift-light `from`/`to`/`flash_at`) take a number or `redline` / `limiter`
//! with an optional offset (`redline - 500`), read from the telemetry each frame, so one design fits cars with
//! different rev limits. A threshold that follows a telemetry figure left at 0 is skipped.
//!
//! **`type = dial`**: `channel`, `radius`, `min`, `max` (required); `start_angle`, `end_angle` (degrees clockwise
//! from 12 o'clock, default −135 and 135); `major`, `minor` (tick spacing in channel units; `minor = 0` for none);
//! `tick_length`, `tick_width`, `tick_color`; numbers every `label_every` (default `major`) from `labels_from`
//! (default `min`), shown as value × `label_scale` with `label_decimals` (so `label_scale = 0.001` prints rpm in
//! thousands), `label_size`, `label_radius` (share of the radius), `label_color`; `warn_from`/`warn_to`/`warn_color`
//! and `red_from`/`red_to`/`red_color` zones (bands at the scale's edge; ticks and numbers in the red zone take its
//! colour); `zone_width`; `face_color`, `rim_color`, `rim_width`; `sweep_color`, `sweep_width`, `sweep_from` (a band
//! under the ticks that fills from `sweep_from`, default `min`, to the value; transparent by default);
//! `needle_color`, `needle_length` (share of the radius), `needle_width`, `hub_color`, `hub_radius`.
//!
//! **`type = bar`**: `channel`, `size = w h`, `min`, `max` (required). It fills upwards when taller than wide, else
//! rightwards. `color`, `track_color`; `low_below` + `low_color` (e.g. a cold engine), `warn_*` and `red_*` zones (the
//! fill takes the zone's colour; the track is tinted where the zones lie); `segments = N` for an LED-style bar;
//! `label` (drawn above the left end of a horizontal bar, below a vertical one), `label_size`, `label_color`;
//! `show_value`, `decimals`, `units` (appended, e.g. `°C`), `value_size`, `value_color` (above the right end of a
//! horizontal bar, above a vertical one).
//!
//! **`type = digital`**: `channel`, `size` (digit height, required); `digits` (minimum number of positions, so the
//! readout does not jump), `decimals`, `align` (`left`/`center`/`right` about `center`), `color`, `ghost_color`
//! (the unlit segments, as on a real LCD; transparent by default), `style` (`segment` or `stroke`), `weight`
//! (stroke or segment thickness as a share of `size`).
//!
//! **`type = gear`**: `size` (required); `color`, `neutral_color`, `reverse_color`, `weight`, `style` (`stroke`
//! default, or `segment`), `background` + `box = w h` for a backing box. Shows `R`, `N` or the gear number.
//!
//! **`type = shift_lights`**: `size = w h`, `from`, `to` (required). `count` lights in a row (default 10); light `i`
//! comes on at `from + i × (to − from) / (count − 1)`, so the first lights at `from` and the last at `to`.
//! `colors` lists colours from the first light to the last, spread evenly over the row (`#00D040 #FFC000 #FF2020`
//! gives thirds); `off_color`; `shape` (`round` or `bar`). At `flash_at` (default `limiter`) or whenever
//! `Telemetry::limiter` is set, every light shows `flash_color`, blinking at `flash_hz` when drawn with
//! [`render_at_time`] (plain [`render`] shows the "on" phase).
//!
//! **`type = text`**: `text`, `size` (required); `color`, `align`, `weight`; `when` = `always` (default),
//! `limiter`, `launch`, `wheelspin`, `reverse`, `neutral` or `redline`, with `off_color` drawn otherwise (transparent
//! by default), for warning lamps such as "LC" or "TC".
//!
//! **`type = panel`**: `size = w h`; `color`, `radius`, `border_color`, `border_width`. A backing plate behind a
//! group of gauges, so they read against any scenery. The built-in font has capitals, digits,
//! `k m h n i x` in lower case, and `- + . , : / ° % ( )`.

mod design;
mod draw;
mod fast;
pub mod font;

pub use design::Dial;
pub use design::{
    Align, Anchor, Bar, Channel, Condition, DigitStyle, Digital, Gauge, GaugeKind, Gear, HudDesign,
    Level, LightShape, Panel, Rgba, ShiftLights, Text, Zone, BUILT_IN, PORSCHE_992_GT3RS,
    TURBO_SPORT,
};
pub use draw::{place, Placement};

/// One frame's values. `Default` is all zero.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Telemetry {
    pub speed_kmh: f32,
    pub rpm: f32,
    pub redline_rpm: f32,
    pub limiter_rpm: f32,
    /// -1 reverse, 0 neutral, 1.. forward.
    pub gear: i32,
    /// 0..1
    pub throttle: f32,
    /// 0..1
    pub brake: f32,
    /// Gauge pressure; 0 for a naturally aspirated car.
    pub boost_bar: f32,
    pub water_temp_c: f32,
    pub oil_temp_c: f32,
    pub oil_pressure_bar: f32,
    pub limiter: bool,
    pub launch: bool,
    pub wheelspin: bool,
}

/// The drawn gauges: the screen rectangle they cover, as premultiplied RGBA8, rows top first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Overlay {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Draws `design` showing `telemetry` for a `screen_width` × `screen_height` screen. The result covers only the
/// panel's rectangle, placed at `x, y` within the screen. Flashing shift lights show their "on" phase; use
/// [`render_at_time`] to make them blink.
pub fn render(
    design: &HudDesign,
    telemetry: &Telemetry,
    screen_width: u32,
    screen_height: u32,
) -> Overlay {
    draw::render(design, telemetry, screen_width, screen_height, 0.0)
}

/// [`render`], with a clock (seconds, any origin) for the parts that blink: shift lights at the limiter.
pub fn render_at_time(
    design: &HudDesign,
    telemetry: &Telemetry,
    screen_width: u32,
    screen_height: u32,
    time_s: f32,
) -> Overlay {
    draw::render(design, telemetry, screen_width, screen_height, time_s)
}

/// Reuses an unchanged dashboard instead of rasterizing and uploading it again. Owns its design so changing
/// the design constructs a fresh cache. Telemetry remains exact; no gauge update rate or precision is reduced.
pub struct HudRenderer {
    design: HudDesign,
    last: Option<(Telemetry, u32, u32, Vec<bool>)>,
}
impl HudRenderer {
    pub fn new(design: HudDesign) -> Self {
        Self { design, last: None }
    }
    /// Call when hiding the HUD: showing it again must upload even if the telemetry did not change.
    pub fn invalidate(&mut self) {
        self.last = None;
    }
    pub fn update(
        &mut self,
        t: &Telemetry,
        width: u32,
        height: u32,
        time_s: f32,
    ) -> Option<Overlay> {
        let phases: Vec<bool> = self
            .design
            .gauges
            .iter()
            .filter_map(|g| match &g.kind {
                GaugeKind::ShiftLights(s) => {
                    Some(s.flashing(t) && (time_s * s.flash_hz * 2.0).floor() as i64 % 2 == 0)
                }
                _ => None,
            })
            .collect();
        let key = (*t, width, height, phases);
        if self.last.as_ref() == Some(&key) {
            return None;
        }
        self.last = Some(key);
        Some(render_at_time(&self.design, t, width, height, time_s))
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn cached_hud_preserves_pixels_and_invalidates_for_telemetry_resize_and_visibility() {
        let d = HudDesign::by_name("porsche-992-gt3rs").unwrap();
        let mut r = HudRenderer::new(d.clone());
        let mut t = Telemetry {
            rpm: 1000.0,
            limiter_rpm: 9000.0,
            redline_rpm: 8500.0,
            ..Default::default()
        };
        assert_eq!(
            r.update(&t, 1280, 720, 0.0),
            Some(render_at_time(&d, &t, 1280, 720, 0.0))
        );
        assert!(r.update(&t, 1280, 720, 10.0).is_none());
        t.speed_kmh = 1.0;
        assert!(r.update(&t, 1280, 720, 10.0).is_some());
        assert!(r.update(&t, 640, 360, 10.0).is_some());
        r.invalidate();
        assert!(r.update(&t, 640, 360, 10.0).is_some());
    }
    #[test]
    fn flashing_shift_lights_still_animate_with_unchanged_telemetry() {
        let d = HudDesign::by_name("porsche-992-gt3rs").unwrap();
        let mut r = HudRenderer::new(d.clone());
        let t = Telemetry {
            rpm: 9000.0,
            redline_rpm: 8500.0,
            limiter_rpm: 9000.0,
            limiter: true,
            ..Default::default()
        };
        let mut previous = r.update(&t, 640, 360, 0.0).unwrap();
        let mut changes = 0;
        for i in 1..20 {
            let now = i as f32 * 0.037;
            let expected = render_at_time(&d, &t, 640, 360, now);
            let actual = r.update(&t, 640, 360, now);
            if expected != previous {
                changes += 1;
                assert_eq!(actual, Some(expected.clone()));
            } else if let Some(actual) = actual {
                assert_eq!(actual, expected);
            }
            previous = expected;
        }
        assert!(changes > 0, "the fixture must actually blink");
    }
}
