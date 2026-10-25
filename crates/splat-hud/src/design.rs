//! The parsed form of a `.hud` design file, and its parser. The format itself is documented on the crate.

use crate::Telemetry;

/// A colour with straight (not premultiplied) alpha, as written in the design file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba([0, 0, 0, 0]);

    /// `#RRGGBB` (opaque) or `#RRGGBBAA`.
    pub fn parse(text: &str) -> Result<Rgba, String> {
        let hex = text.strip_prefix('#').ok_or(format!("`{text}` is not a colour; write #RRGGBB or #RRGGBBAA"))?;
        if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("`{text}` is not a colour; write #RRGGBB or #RRGGBBAA"));
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
        Ok(Rgba([byte(0), byte(2), byte(4), if hex.len() == 8 { byte(6) } else { 255 }]))
    }

    pub fn visible(self) -> bool {
        self.0[3] > 0
    }

    /// The same colour with its alpha multiplied by `factor` (0..1).
    pub fn faded(self, factor: f32) -> Rgba {
        let [r, g, b, a] = self.0;
        Rgba([r, g, b, (a as f32 * factor.clamp(0.0, 1.0)).round() as u8])
    }

    /// Each channel moved `amount` (0..1) of the way to white.
    pub fn lightened(self, amount: f32) -> Rgba {
        let [r, g, b, a] = self.0;
        let up = |c: u8| (c as f32 + (255.0 - c as f32) * amount).round() as u8;
        Rgba([up(r), up(g), up(b), a])
    }
}

/// Which corner, edge or centre of the screen the panel sits against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// A telemetry value a gauge reads, named as the [`Telemetry`] field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    SpeedKmh,
    Rpm,
    BoostBar,
    WaterTempC,
    OilTempC,
    OilPressureBar,
    Throttle,
    Brake,
    Gear,
}

impl Channel {
    fn parse(text: &str) -> Result<Channel, String> {
        Ok(match text {
            "speed_kmh" => Channel::SpeedKmh,
            "rpm" => Channel::Rpm,
            "boost_bar" => Channel::BoostBar,
            "water_temp_c" => Channel::WaterTempC,
            "oil_temp_c" => Channel::OilTempC,
            "oil_pressure_bar" => Channel::OilPressureBar,
            "throttle" => Channel::Throttle,
            "brake" => Channel::Brake,
            "gear" => Channel::Gear,
            other => {
                return Err(format!(
                    "`{other}` is not a channel; use speed_kmh, rpm, boost_bar, water_temp_c, oil_temp_c, \
                     oil_pressure_bar, throttle, brake or gear"
                ))
            }
        })
    }

    /// The channel's value this frame. Speed reads as its magnitude (a speedometer does not go negative in
    /// reverse), and a non-finite value reads as 0.
    pub fn read(self, t: &Telemetry) -> f32 {
        let v = match self {
            Channel::SpeedKmh => t.speed_kmh.abs(),
            Channel::Rpm => t.rpm,
            Channel::BoostBar => t.boost_bar,
            Channel::WaterTempC => t.water_temp_c,
            Channel::OilTempC => t.oil_temp_c,
            Channel::OilPressureBar => t.oil_pressure_bar,
            Channel::Throttle => t.throttle,
            Channel::Brake => t.brake,
            Channel::Gear => t.gear as f32,
        };
        if v.is_finite() {
            v
        } else {
            0.0
        }
    }
}

/// A threshold that is either a fixed number or follows the car's telemetry: `redline`, `limiter`, optionally
/// with an offset (`redline - 500`). This lets one design serve cars with different rev limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Level {
    Fixed(f32),
    Redline(f32),
    Limiter(f32),
}

impl Level {
    fn parse(text: &str) -> Result<Level, String> {
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        for (word, make) in [("redline", Level::Redline as fn(f32) -> Level), ("limiter", Level::Limiter)] {
            if let Some(rest) = compact.strip_prefix(word) {
                if rest.is_empty() {
                    return Ok(make(0.0));
                }
                let offset: f32 = rest
                    .strip_prefix('+')
                    .unwrap_or(rest)
                    .parse()
                    .map_err(|_| format!("`{text}`: write {word}, {word} + N or {word} - N"))?;
                return Ok(make(offset));
            }
        }
        parse_number(text).map(Level::Fixed)
    }

    /// The threshold's value this frame, or `None` when it follows a telemetry figure the game left at zero.
    pub fn resolve(self, t: &Telemetry) -> Option<f32> {
        match self {
            Level::Fixed(v) => Some(v),
            Level::Redline(o) => (t.redline_rpm > 0.0).then_some(t.redline_rpm + o),
            Level::Limiter(o) => (t.limiter_rpm > 0.0).then_some(t.limiter_rpm + o),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// When a `text` gauge is lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    Always,
    Limiter,
    Launch,
    Wheelspin,
    Reverse,
    Neutral,
    /// rpm at or above the telemetry redline.
    Redline,
}

impl Condition {
    pub fn holds(self, t: &Telemetry) -> bool {
        match self {
            Condition::Always => true,
            Condition::Limiter => t.limiter,
            Condition::Launch => t.launch,
            Condition::Wheelspin => t.wheelspin,
            Condition::Reverse => t.gear < 0,
            Condition::Neutral => t.gear == 0,
            Condition::Redline => t.redline_rpm > 0.0 && t.rpm >= t.redline_rpm,
        }
    }
}

/// A coloured band of a dial's scale or a bar's track: from `from` up to `to` (the gauge's max when absent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub from: Level,
    pub to: Option<Level>,
    pub color: Rgba,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dial {
    pub channel: Channel,
    pub center: [f32; 2],
    pub radius: f32,
    pub min: f32,
    pub max: f32,
    /// Degrees clockwise from 12 o'clock where `min` and `max` sit.
    pub start_angle: f32,
    pub end_angle: f32,
    pub major: f32,
    pub minor: f32,
    pub label_every: f32,
    pub labels_from: f32,
    pub label_scale: f32,
    pub label_decimals: usize,
    pub label_size: f32,
    pub label_radius: f32,
    pub label_color: Rgba,
    pub face_color: Rgba,
    pub rim_color: Rgba,
    pub rim_width: f32,
    pub tick_color: Rgba,
    pub tick_width: f32,
    pub tick_length: f32,
    pub warn: Option<Zone>,
    pub red: Option<Zone>,
    pub zone_width: f32,
    pub sweep_color: Rgba,
    pub sweep_width: f32,
    /// Where the sweep starts (default `min`); 0 on a boost gauge makes vacuum sweep backwards.
    pub sweep_from: f32,
    pub needle_color: Rgba,
    pub needle_length: f32,
    pub needle_width: f32,
    pub hub_color: Rgba,
    pub hub_radius: f32,
}

impl Dial {
    /// The needle angle for `value`, in degrees clockwise from 12 o'clock, clamped to the scale.
    pub fn angle_deg(&self, value: f32) -> f32 {
        let span = self.max - self.min;
        let f = if span.abs() > f32::EPSILON { ((value - self.min) / span).clamp(0.0, 1.0) } else { 0.0 };
        let f = if f.is_finite() { f } else { 0.0 };
        self.start_angle + f * (self.end_angle - self.start_angle)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub channel: Channel,
    pub center: [f32; 2],
    /// Width and height; the bar fills upwards when it is taller than wide, else rightwards.
    pub size: [f32; 2],
    pub min: f32,
    pub max: f32,
    pub color: Rgba,
    pub track_color: Rgba,
    pub low_below: Option<f32>,
    pub low_color: Rgba,
    pub warn: Option<Zone>,
    pub red: Option<Zone>,
    pub segments: u32,
    pub label: String,
    pub label_size: f32,
    pub label_color: Rgba,
    pub show_value: bool,
    pub decimals: usize,
    /// Appended to the value, e.g. `°C`.
    pub units: String,
    pub value_size: f32,
    pub value_color: Rgba,
}

impl Bar {
    pub fn vertical(&self) -> bool {
        self.size[1] > self.size[0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigitStyle {
    /// Seven-segment digits.
    Segment,
    /// The stroked vector font.
    Stroke,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Digital {
    pub channel: Channel,
    pub center: [f32; 2],
    /// Digit height.
    pub size: f32,
    /// Minimum number of digit positions; unused leading positions show only the ghost segments.
    pub digits: usize,
    pub decimals: usize,
    pub color: Rgba,
    pub ghost_color: Rgba,
    pub align: Align,
    pub style: DigitStyle,
    pub weight: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gear {
    pub center: [f32; 2],
    pub size: f32,
    pub color: Rgba,
    pub neutral_color: Rgba,
    pub reverse_color: Rgba,
    pub weight: f32,
    pub style: DigitStyle,
    pub background: Rgba,
    pub box_size: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightShape {
    Round,
    Bar,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShiftLights {
    pub center: [f32; 2],
    pub size: [f32; 2],
    pub count: u32,
    /// rpm at which the first light comes on.
    pub from: Level,
    /// rpm at which the last light comes on.
    pub to: Level,
    /// Colours from the first light to the last, spread evenly over `count` lights.
    pub colors: Vec<Rgba>,
    pub off_color: Rgba,
    pub flash_color: Rgba,
    /// All lights flash at or above this rpm, and whenever the telemetry says the limiter is cutting.
    pub flash_at: Level,
    pub flash_hz: f32,
    pub shape: LightShape,
}

impl ShiftLights {
    /// How many lights are lit at `rpm`: light `i` (from 0) comes on at `from + i × (to − from) / (count − 1)`.
    pub fn lit(&self, rpm: f32, t: &Telemetry) -> u32 {
        let (Some(from), Some(to)) = (self.from.resolve(t), self.to.resolve(t)) else { return 0 };
        if self.count == 0 || !rpm.is_finite() || rpm < from {
            return 0;
        }
        if self.count == 1 || to <= from {
            return self.count;
        }
        let step = (to - from) / (self.count - 1) as f32;
        (((rpm - from) / step + 1e-4).floor() as u32 + 1).min(self.count)
    }

    /// Whether the lights are in their flashing state this frame (the limiter, or rpm at `flash_at`).
    pub fn flashing(&self, t: &Telemetry) -> bool {
        t.limiter || self.flash_at.resolve(t).is_some_and(|at| t.rpm >= at)
    }

    /// The colour of light `i` when lit.
    pub fn color_of(&self, i: u32) -> Rgba {
        if self.colors.is_empty() {
            return Rgba([255, 255, 255, 255]);
        }
        let k = (i as usize * self.colors.len()) / self.count.max(1) as usize;
        self.colors[k.min(self.colors.len() - 1)]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub center: [f32; 2],
    pub text: String,
    pub size: f32,
    pub color: Rgba,
    pub off_color: Rgba,
    pub when: Condition,
    pub align: Align,
    pub weight: f32,
}

/// A plain rounded rectangle, to back a group of gauges.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub center: [f32; 2],
    pub size: [f32; 2],
    pub color: Rgba,
    pub radius: f32,
    pub border_color: Rgba,
    pub border_width: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GaugeKind {
    Panel(Panel),
    Dial(Dial),
    Bar(Bar),
    Digital(Digital),
    Gear(Gear),
    ShiftLights(ShiftLights),
    Text(Text),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gauge {
    /// The part after `gauge.` in the section name.
    pub name: String,
    pub kind: GaugeKind,
}

/// A parsed `.hud` design: the panel's placement and its gauges in drawing order.
#[derive(Debug, Clone, PartialEq)]
pub struct HudDesign {
    pub name: String,
    pub anchor: Anchor,
    /// Panel width and height in design units; gauge positions are measured from its top-left corner.
    pub size: [f32; 2],
    /// Panel height as a share of the screen height.
    pub height: f32,
    /// Gap between the panel and the screen edges it is anchored to, as a share of the screen height.
    pub margin: f32,
    pub background: Rgba,
    pub corner_radius: f32,
    pub gauges: Vec<Gauge>,
}

pub const PORSCHE_992_GT3RS: &str = include_str!("../designs/porsche-992-gt3rs.hud");
pub const TURBO_SPORT: &str = include_str!("../designs/turbo-sport.hud");

/// Built-in designs by file stem.
pub const BUILT_IN: [(&str, &str); 2] = [("porsche-992-gt3rs", PORSCHE_992_GT3RS), ("turbo-sport", TURBO_SPORT)];

impl HudDesign {
    /// The built-in Porsche 911 GT3 RS (992) cluster.
    pub fn porsche_992_gt3rs() -> HudDesign {
        HudDesign::parse(PORSCHE_992_GT3RS).expect("the built-in design parses")
    }

    /// A built-in design by its file stem (`porsche-992-gt3rs`, `turbo-sport`).
    pub fn by_name(name: &str) -> Option<HudDesign> {
        BUILT_IN.iter().find(|(stem, _)| *stem == name).map(|(_, text)| HudDesign::parse(text).expect("the built-in design parses"))
    }

    pub fn parse(text: &str) -> Result<HudDesign, String> {
        let sections = split_sections(text)?;
        let mut hud = None;
        let mut gauges = Vec::new();
        for section in &sections {
            let mut f = Fields::new(section);
            if section.name == "hud" {
                if hud.is_some() {
                    return Err(format!("line {}: a second [hud] section", section.line));
                }
                hud = Some(parse_hud(&mut f)?);
            } else if let Some(name) = section.name.strip_prefix("gauge.") {
                if name.is_empty() || gauges.iter().any(|g: &Gauge| g.name == name) {
                    return Err(format!("line {}: [{}] needs a unique name after `gauge.`", section.line, section.name));
                }
                gauges.push(Gauge { name: name.to_string(), kind: parse_gauge(&mut f)? });
            } else {
                return Err(format!("line {}: unknown section [{}]; use [hud] or [gauge.<name>]", section.line, section.name));
            }
            f.finish()?;
        }
        let mut design = hud.ok_or("missing [hud] section")?;
        design.gauges = gauges;
        Ok(design)
    }
}

struct Entry {
    key: String,
    line: usize,
    value: String,
}

struct Section {
    name: String,
    line: usize,
    entries: Vec<Entry>,
}

/// Strips a comment: a `#` at the start of the line, or a `#` followed by whitespace, another `#` or the end of the
/// line. A `#` directly followed by anything else (a colour such as `#FF0000`) is part of the value.
fn strip_comment(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    if trimmed.starts_with('#') {
        return "";
    }
    let bytes = raw.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' {
            let next = bytes.get(i + 1).copied();
            if next.is_none() || next.is_some_and(|n| n.is_ascii_whitespace() || n == b'#') {
                return &raw[..i];
            }
        }
    }
    raw
}

fn split_sections(text: &str) -> Result<Vec<Section>, String> {
    let mut sections: Vec<Section> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let name = name.trim().to_string();
            if sections.iter().any(|s| s.name == name) {
                return Err(format!("line {line_no}: section [{name}] appears twice"));
            }
            sections.push(Section { name, line: line_no, entries: Vec::new() });
            continue;
        }
        let (key, value) = line.split_once('=').ok_or(format!("line {line_no}: expected `key = value`"))?;
        let key = key.trim().to_string();
        let section = sections.last_mut().ok_or(format!("line {line_no}: `{key}` comes before any [section]"))?;
        if section.entries.iter().any(|e| e.key == key) {
            return Err(format!("line {line_no}: [{}] {key} is set twice", section.name));
        }
        section.entries.push(Entry { key, line: line_no, value: value.trim().to_string() });
    }
    Ok(sections)
}

fn parse_number(text: &str) -> Result<f32, String> {
    match text.parse::<f32>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(format!("`{text}` is not a number")),
    }
}

/// Reads one section's keys, remembering which were used so the rest can be reported as unknown.
struct Fields<'a> {
    section: &'a Section,
    used: Vec<bool>,
}

impl<'a> Fields<'a> {
    fn new(section: &'a Section) -> Self {
        Fields { section, used: vec![false; section.entries.len()] }
    }

    fn raw(&mut self, key: &str) -> Option<(usize, &'a str)> {
        let i = self.section.entries.iter().position(|e| e.key == key)?;
        self.used[i] = true;
        let e = &self.section.entries[i];
        Some((e.line, e.value.as_str()))
    }

    fn err(&self, line: usize, key: &str, message: impl std::fmt::Display) -> String {
        format!("line {line}: [{}] {key}: {message}", self.section.name)
    }

    fn get<T>(&mut self, key: &str, parse: impl Fn(&str) -> Result<T, String>) -> Result<Option<T>, String> {
        match self.raw(key) {
            None => Ok(None),
            Some((line, value)) => parse(value).map(Some).map_err(|m| self.err(line, key, m)),
        }
    }

    fn require<T>(&mut self, key: &str, parse: impl Fn(&str) -> Result<T, String>) -> Result<T, String> {
        self.get(key, parse)?.ok_or_else(|| format!("line {}: [{}] missing `{key}`", self.section.line, self.section.name))
    }

    fn number(&mut self, key: &str, default: f32) -> Result<f32, String> {
        Ok(self.get(key, parse_number)?.unwrap_or(default))
    }

    fn positive(&mut self, key: &str, default: f32) -> Result<f32, String> {
        let v = self.number(key, default)?;
        if v > 0.0 {
            Ok(v)
        } else {
            let line = self.raw(key).map_or(self.section.line, |(l, _)| l);
            Err(self.err(line, key, "must be positive"))
        }
    }

    fn required_positive(&mut self, key: &str) -> Result<f32, String> {
        let v = self.require(key, parse_number)?;
        if v > 0.0 {
            Ok(v)
        } else {
            let line = self.raw(key).map_or(self.section.line, |(l, _)| l);
            Err(self.err(line, key, "must be positive"))
        }
    }

    fn count(&mut self, key: &str, default: usize) -> Result<usize, String> {
        Ok(self.get(key, |v| v.parse::<usize>().map_err(|_| format!("`{v}` is not a whole number")))?.unwrap_or(default))
    }

    fn pair(&mut self, key: &str) -> Result<[f32; 2], String> {
        self.require(key, parse_pair)
    }

    fn color(&mut self, key: &str, default: Rgba) -> Result<Rgba, String> {
        Ok(self.get(key, Rgba::parse)?.unwrap_or(default))
    }

    fn flag(&mut self, key: &str, default: bool) -> Result<bool, String> {
        Ok(self
            .get(key, |v| match v {
                "true" | "yes" | "on" => Ok(true),
                "false" | "no" | "off" => Ok(false),
                other => Err(format!("`{other}` is not true or false")),
            })?
            .unwrap_or(default))
    }

    fn align(&mut self, key: &str, default: Align) -> Result<Align, String> {
        Ok(self
            .get(key, |v| match v {
                "left" => Ok(Align::Left),
                "center" | "centre" => Ok(Align::Center),
                "right" => Ok(Align::Right),
                other => Err(format!("`{other}` is not left, center or right")),
            })?
            .unwrap_or(default))
    }

    fn style(&mut self, default: DigitStyle) -> Result<DigitStyle, String> {
        Ok(self
            .get("style", |v| match v {
                "segment" => Ok(DigitStyle::Segment),
                "stroke" => Ok(DigitStyle::Stroke),
                other => Err(format!("`{other}` is not segment or stroke")),
            })?
            .unwrap_or(default))
    }

    /// A zone from `<prefix>_from`, `<prefix>_to` and `<prefix>_color`; absent when `<prefix>_from` is.
    fn zone(&mut self, prefix: &str, default_color: Rgba) -> Result<Option<Zone>, String> {
        let from = self.get(&format!("{prefix}_from"), Level::parse)?;
        let to = self.get(&format!("{prefix}_to"), Level::parse)?;
        let color = self.color(&format!("{prefix}_color"), default_color)?;
        Ok(from.map(|from| Zone { from, to, color }))
    }

    fn finish(self) -> Result<(), String> {
        let unknown: Vec<String> = self
            .section
            .entries
            .iter()
            .zip(&self.used)
            .filter(|(_, used)| !**used)
            .map(|(e, _)| format!("line {}: [{}] unknown key `{}`", e.line, self.section.name, e.key))
            .collect();
        if unknown.is_empty() {
            Ok(())
        } else {
            Err(unknown.join("; "))
        }
    }
}

fn parse_pair(text: &str) -> Result<[f32; 2], String> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    match parts.as_slice() {
        [a, b] => Ok([parse_number(a)?, parse_number(b)?]),
        _ => Err(format!("`{text}` should be two numbers, `x y`")),
    }
}

const WHITE: Rgba = Rgba([255, 255, 255, 255]);
const RED: Rgba = Rgba([230, 30, 30, 255]);
const AMBER: Rgba = Rgba([255, 180, 0, 255]);

fn parse_hud(f: &mut Fields) -> Result<HudDesign, String> {
    let anchor = f.require("anchor", |v| {
        Ok(match v {
            "top-left" => Anchor::TopLeft,
            "top-center" => Anchor::TopCenter,
            "top-right" => Anchor::TopRight,
            "center-left" => Anchor::CenterLeft,
            "center" => Anchor::Center,
            "center-right" => Anchor::CenterRight,
            "bottom-left" => Anchor::BottomLeft,
            "bottom-center" => Anchor::BottomCenter,
            "bottom-right" => Anchor::BottomRight,
            other => return Err(format!("`{other}` is not top-|center-|bottom- left|center|right (or center)")),
        })
    })?;
    let size = f.pair("size")?;
    if size[0] <= 0.0 || size[1] <= 0.0 {
        return Err(format!("line {}: [hud] size: both sides must be positive", f.section.line));
    }
    let height = f.required_positive("height")?;
    Ok(HudDesign {
        name: f.require("name", |v| Ok(v.to_string()))?,
        anchor,
        size,
        height,
        margin: f.number("margin", 0.02)?.max(0.0),
        background: f.color("background", Rgba::TRANSPARENT)?,
        corner_radius: f.number("corner_radius", 0.0)?.max(0.0),
        gauges: Vec::new(),
    })
}

fn parse_gauge(f: &mut Fields) -> Result<GaugeKind, String> {
    let kind = f.require("type", |v| Ok(v.to_string()))?;
    let center = f.pair("center")?;
    Ok(match kind.as_str() {
        "dial" => {
            let radius = f.required_positive("radius")?;
            let min = f.require("min", parse_number)?;
            let max = f.require("max", parse_number)?;
            if max <= min {
                return Err(format!("line {}: [{}] max must be above min", f.section.line, f.section.name));
            }
            let major = f.positive("major", (max - min) / 10.0)?;
            GaugeKind::Dial(Dial {
                channel: f.require("channel", Channel::parse)?,
                center,
                radius,
                min,
                max,
                start_angle: f.number("start_angle", -135.0)?,
                end_angle: f.number("end_angle", 135.0)?,
                major,
                minor: f.number("minor", 0.0)?.max(0.0),
                label_every: f.positive("label_every", major)?,
                labels_from: f.number("labels_from", min)?,
                label_scale: f.number("label_scale", 1.0)?,
                label_decimals: f.count("label_decimals", 0)?,
                label_size: f.number("label_size", radius * 0.14)?.max(0.0),
                label_radius: f.positive("label_radius", 0.68)?,
                label_color: f.color("label_color", WHITE)?,
                face_color: f.color("face_color", Rgba([12, 12, 14, 235]))?,
                rim_color: f.color("rim_color", Rgba([90, 90, 96, 255]))?,
                rim_width: f.number("rim_width", radius * 0.03)?.max(0.0),
                tick_color: f.color("tick_color", WHITE)?,
                tick_width: f.positive("tick_width", radius * 0.022)?,
                tick_length: f.positive("tick_length", radius * 0.12)?,
                warn: f.zone("warn", AMBER)?,
                red: f.zone("red", RED)?,
                zone_width: f.number("zone_width", radius * 0.05)?.max(0.0),
                sweep_color: f.color("sweep_color", Rgba::TRANSPARENT)?,
                sweep_width: f.positive("sweep_width", radius * 0.12)?,
                sweep_from: f.number("sweep_from", min)?,
                needle_color: f.color("needle_color", Rgba([255, 60, 30, 255]))?,
                needle_length: f.positive("needle_length", 0.9)?,
                needle_width: f.positive("needle_width", radius * 0.045)?,
                hub_color: f.color("hub_color", Rgba([30, 30, 34, 255]))?,
                hub_radius: f.number("hub_radius", radius * 0.09)?.max(0.0),
            })
        }
        "bar" => {
            let size = f.pair("size")?;
            let min = f.require("min", parse_number)?;
            let max = f.require("max", parse_number)?;
            if max <= min {
                return Err(format!("line {}: [{}] max must be above min", f.section.line, f.section.name));
            }
            let thickness = size[0].min(size[1]);
            GaugeKind::Bar(Bar {
                channel: f.require("channel", Channel::parse)?,
                center,
                size,
                min,
                max,
                color: f.color("color", WHITE)?,
                track_color: f.color("track_color", Rgba([255, 255, 255, 40]))?,
                low_below: f.get("low_below", parse_number)?,
                low_color: f.color("low_color", Rgba([60, 140, 255, 255]))?,
                warn: f.zone("warn", AMBER)?,
                red: f.zone("red", RED)?,
                segments: f.count("segments", 0)? as u32,
                label: f.get("label", |v| Ok(v.to_string()))?.unwrap_or_default(),
                label_size: f.number("label_size", thickness * 1.2)?.max(0.0),
                label_color: f.color("label_color", WHITE)?,
                show_value: f.flag("show_value", false)?,
                decimals: f.count("decimals", 0)?,
                units: f.get("units", |v| Ok(v.to_string()))?.unwrap_or_default(),
                value_size: f.number("value_size", thickness * 1.4)?.max(0.0),
                value_color: f.color("value_color", WHITE)?,
            })
        }
        "digital" => GaugeKind::Digital(Digital {
            channel: f.require("channel", Channel::parse)?,
            center,
            size: f.required_positive("size")?,
            digits: f.count("digits", 1)?,
            decimals: f.count("decimals", 0)?,
            color: f.color("color", WHITE)?,
            ghost_color: f.color("ghost_color", Rgba::TRANSPARENT)?,
            align: f.align("align", Align::Center)?,
            style: f.style(DigitStyle::Segment)?,
            weight: f.positive("weight", 0.13)?,
        }),
        "gear" => GaugeKind::Gear(Gear {
            center,
            size: f.required_positive("size")?,
            color: f.color("color", WHITE)?,
            neutral_color: f.color("neutral_color", Rgba([80, 220, 90, 255]))?,
            reverse_color: f.color("reverse_color", AMBER)?,
            weight: f.positive("weight", 0.16)?,
            style: f.style(DigitStyle::Stroke)?,
            background: f.color("background", Rgba::TRANSPARENT)?,
            box_size: f.get("box", parse_pair)?.unwrap_or([0.0, 0.0]),
        }),
        "shift_lights" => {
            let count = f.count("count", 10)? as u32;
            if count == 0 {
                return Err(format!("line {}: [{}] count must be at least 1", f.section.line, f.section.name));
            }
            GaugeKind::ShiftLights(ShiftLights {
                center,
                size: f.pair("size")?,
                count,
                from: f.require("from", Level::parse)?,
                to: f.require("to", Level::parse)?,
                colors: f
                    .get("colors", |v| v.split_whitespace().map(Rgba::parse).collect::<Result<Vec<_>, _>>())?
                    .unwrap_or_else(|| vec![Rgba([40, 220, 60, 255]), AMBER, RED]),
                off_color: f.color("off_color", Rgba([255, 255, 255, 28]))?,
                flash_color: f.color("flash_color", Rgba([40, 120, 255, 255]))?,
                flash_at: f.get("flash_at", Level::parse)?.unwrap_or(Level::Limiter(0.0)),
                flash_hz: f.positive("flash_hz", 8.0)?,
                shape: f
                    .get("shape", |v| match v {
                        "round" => Ok(LightShape::Round),
                        "bar" => Ok(LightShape::Bar),
                        other => Err(format!("`{other}` is not round or bar")),
                    })?
                    .unwrap_or(LightShape::Round),
            })
        }
        "panel" => GaugeKind::Panel(Panel {
            center,
            size: f.pair("size")?,
            color: f.color("color", Rgba([0, 0, 0, 120]))?,
            radius: f.number("radius", 0.0)?.max(0.0),
            border_color: f.color("border_color", Rgba::TRANSPARENT)?,
            border_width: f.number("border_width", 1.0)?.max(0.0),
        }),
        "text" => GaugeKind::Text(Text {
            center,
            text: f.require("text", |v| Ok(v.to_string()))?,
            size: f.required_positive("size")?,
            color: f.color("color", WHITE)?,
            off_color: f.color("off_color", Rgba::TRANSPARENT)?,
            when: f
                .get("when", |v| {
                    Ok(match v {
                        "always" => Condition::Always,
                        "limiter" => Condition::Limiter,
                        "launch" => Condition::Launch,
                        "wheelspin" => Condition::Wheelspin,
                        "reverse" => Condition::Reverse,
                        "neutral" => Condition::Neutral,
                        "redline" => Condition::Redline,
                        other => {
                            return Err(format!(
                                "`{other}` is not always, limiter, launch, wheelspin, reverse, neutral or redline"
                            ))
                        }
                    })
                })?
                .unwrap_or(Condition::Always),
            align: f.align("align", Align::Center)?,
            weight: f.positive("weight", 0.12)?,
        }),
        other => {
            let line = f.raw("type").map_or(f.section.line, |(l, _)| l);
            return Err(f.err(line, "type", format!("`{other}` is not dial, bar, digital, gear, shift_lights, text or panel")));
        }
    })
}
