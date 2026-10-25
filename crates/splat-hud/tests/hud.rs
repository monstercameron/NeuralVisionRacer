use splat_hud::*;

const MINIMAL: &str = "\
# a comment
[hud]
name = Test
anchor = bottom-right
size = 200 100   # design units
height = 0.25
margin = 0.02

[gauge.tach]
type = dial
channel = rpm
center = 100 50
radius = 45
min = 0
max = 8000
start_angle = -120
end_angle = 120
major = 1000
minor = 500
red_from = 7000
needle_color = #FF3A1E  # orange
face_color = #000000C0

[gauge.shift]
type = shift_lights
center = 100 6
size = 100 8
count = 5
from = 6000
to = 7000
colors = #00FF00 #FFFF00 #FF0000
";

fn gauge<'a>(design: &'a HudDesign, name: &str) -> &'a GaugeKind {
    &design.gauges.iter().find(|g| g.name == name).unwrap_or_else(|| panic!("no gauge {name}")).kind
}

fn dial(design: &HudDesign, name: &str) -> Dial {
    match gauge(design, name) {
        GaugeKind::Dial(d) => d.clone(),
        other => panic!("{name} is not a dial: {other:?}"),
    }
}

#[test]
fn a_good_file_parses() {
    let design = HudDesign::parse(MINIMAL).unwrap();
    assert_eq!(design.name, "Test");
    assert_eq!(design.anchor, Anchor::BottomRight);
    assert_eq!(design.size, [200.0, 100.0]);
    assert_eq!(design.gauges.len(), 2);
    let tach = dial(&design, "tach");
    assert_eq!(tach.channel, Channel::Rpm);
    assert_eq!(tach.needle_color, Rgba([0xFF, 0x3A, 0x1E, 0xFF]), "a trailing comment after a colour is stripped");
    assert_eq!(tach.face_color, Rgba([0, 0, 0, 0xC0]));
    assert_eq!(tach.red.unwrap().from, Level::Fixed(7000.0));
}

#[test]
fn unknown_keys_are_errors_that_name_the_line_and_key() {
    let text = MINIMAL.replace("minor = 500", "minor = 500\nneedle_glow = 3");
    let err = HudDesign::parse(&text).unwrap_err();
    let line = text.lines().position(|l| l.starts_with("needle_glow")).unwrap() + 1;
    assert!(err.contains(&format!("line {line}")), "{err}");
    assert!(err.contains("unknown key `needle_glow`") && err.contains("[gauge.tach]"), "{err}");

    let err = HudDesign::parse(&format!("{MINIMAL}\n[gauge.x]\ntype = sundial\ncenter = 0 0\n")).unwrap_err();
    assert!(err.contains("type") && err.contains("sundial"), "{err}");
    let err = HudDesign::parse(&format!("{MINIMAL}\n[speedo]\n")).unwrap_err();
    assert!(err.contains("unknown section [speedo]"), "{err}");
    let err = HudDesign::parse(&MINIMAL.replace("radius = 45", "radius = 45\nradius = 50")).unwrap_err();
    assert!(err.contains("radius is set twice"), "{err}");
    let err = HudDesign::parse(&MINIMAL.replace("radius = 45\n", "")).unwrap_err();
    assert!(err.contains("missing `radius`"), "{err}");
}

#[test]
fn bad_colours_are_errors() {
    for bad in ["#FF3A1", "FF3A1E", "#GG3A1E", "#FF3A1E0"] {
        let text = MINIMAL.replace("#FF3A1E  # orange", bad);
        let err = HudDesign::parse(&text).unwrap_err();
        let line = text.lines().position(|l| l.starts_with("needle_color")).unwrap() + 1;
        assert!(err.contains(&format!("line {line}")) && err.contains("needle_color") && err.contains("colour"), "{bad}: {err}");
    }
    assert_eq!(Rgba::parse("#102030").unwrap(), Rgba([0x10, 0x20, 0x30, 0xFF]));
}

#[test]
fn dial_values_map_to_angles() {
    let tach = dial(&HudDesign::parse(MINIMAL).unwrap(), "tach");
    assert_eq!(tach.angle_deg(0.0), -120.0);
    assert_eq!(tach.angle_deg(4000.0), 0.0);
    assert_eq!(tach.angle_deg(8000.0), 120.0);
    assert!((tach.angle_deg(2000.0) - -60.0).abs() < 1e-4);
    assert_eq!(tach.angle_deg(-500.0), -120.0, "clamped below");
    assert_eq!(tach.angle_deg(12000.0), 120.0, "clamped above");
    assert_eq!(tach.angle_deg(f32::NAN), -120.0);

    let boost = dial(&HudDesign::by_name("turbo-sport").unwrap(), "boost");
    assert_eq!((boost.min, boost.max), (-1.0, 2.0));
    assert!((boost.angle_deg(0.5) - 0.0).abs() < 1e-4, "0.5 bar sits at 12 o'clock on a -1..2 dial");
}

#[test]
fn shift_lights_come_on_progressively() {
    let design = HudDesign::parse(MINIMAL).unwrap();
    let GaugeKind::ShiftLights(lights) = gauge(&design, "shift") else { panic!() };
    let t = Telemetry::default();
    // 5 lights from 6,000 to 7,000: one every 250 rpm.
    for (rpm, lit) in [(0.0, 0), (5999.0, 0), (6000.0, 1), (6249.0, 1), (6250.0, 2), (6600.0, 3), (6750.0, 4), (7000.0, 5), (9000.0, 5)] {
        assert_eq!(lights.lit(rpm, &t), lit, "at {rpm} rpm");
    }
    assert_eq!(lights.color_of(0), Rgba([0, 255, 0, 255]));
    assert_eq!(lights.color_of(4), Rgba([255, 0, 0, 255]));

    // The turbo design's lights follow the telemetry redline.
    let turbo = HudDesign::by_name("turbo-sport").unwrap();
    let GaugeKind::ShiftLights(lights) = gauge(&turbo, "shift") else { panic!() };
    let t = Telemetry { redline_rpm: 7000.0, limiter_rpm: 7200.0, ..Default::default() };
    assert_eq!(lights.lit(4999.0, &t), 0);
    assert_eq!(lights.lit(5000.0, &t), 1);
    assert_eq!(lights.lit(6800.0, &t), lights.count);
    assert_eq!(lights.lit(6000.0, &Telemetry::default()), 0, "no redline known: no lights");
    assert!(lights.flashing(&Telemetry { rpm: 7150.0, ..t }));
    assert!(!lights.flashing(&Telemetry { rpm: 6900.0, ..t }));
}

#[test]
fn render_covers_the_panel_at_its_anchor() {
    let design = HudDesign::parse(MINIMAL).unwrap();
    let t = Telemetry { rpm: 5000.0, ..Default::default() };
    let o = render(&design, &t, 1280, 720);
    // 25 % of 720 = 180 px tall, so 1.8 px per unit: 360 × 180, 14.4 px from the bottom-right corner.
    assert_eq!((o.width, o.height), (360, 180));
    assert_eq!((o.x, o.y), (1280 - 360 - 14, 720 - 180 - 14));
    assert_eq!(o.rgba.len(), (o.width * o.height * 4) as usize);
    assert!(o.rgba.chunks(4).any(|p| p[3] > 0), "something is drawn");
    // Premultiplied: no channel exceeds alpha.
    assert!(o.rgba.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
    // The corners outside the round dial stay clear.
    assert_eq!(&o.rgba[0..4], &[0, 0, 0, 0]);

    // Same look at any resolution: the panel scales with the screen height.
    let big = render(&design, &t, 2560, 1440);
    assert_eq!((big.width, big.height), (720, 360));
    // A portrait screen shrinks the panel to fit its width.
    let narrow = render(&design, &t, 300, 1000);
    assert!(narrow.x + narrow.width <= 300 && narrow.width > 0);
    // Nothing to draw on an empty screen, and no panic.
    assert_eq!(render(&design, &t, 0, 0).rgba.len(), 0);
}

#[test]
fn the_needle_moves_with_the_value() {
    let design = HudDesign::parse(MINIMAL).unwrap();
    let low = render(&design, &Telemetry { rpm: 1000.0, ..Default::default() }, 1280, 720);
    let high = render(&design, &Telemetry { rpm: 7000.0, ..Default::default() }, 1280, 720);
    let same = render(&design, &Telemetry { rpm: 1000.0, ..Default::default() }, 1280, 720);
    assert_ne!(low.rgba, high.rgba);
    assert_eq!(low.rgba, same.rgba, "render is stateless");
}

#[test]
fn both_built_ins_parse_and_render() {
    let porsche = HudDesign::porsche_992_gt3rs();
    assert_eq!(porsche, HudDesign::by_name("porsche-992-gt3rs").unwrap());
    let tach = dial(&porsche, "tach");
    assert_eq!((tach.min, tach.max, tach.labels_from, tach.label_scale), (0.0, 9000.0, 1000.0, 0.001));
    let red = tach.red.unwrap();
    assert_eq!((red.from, red.to), (Level::Fixed(8500.0), Some(Level::Fixed(9000.0))));
    assert!(
        !porsche.gauges.iter().any(|g| matches!(&g.kind, GaugeKind::Dial(d) if d.channel == Channel::BoostBar)),
        "the GT3 RS is naturally aspirated"
    );
    let turbo = HudDesign::by_name("turbo-sport").unwrap();
    assert_eq!(dial(&turbo, "boost").channel, Channel::BoostBar);
    assert!(HudDesign::by_name("nope").is_none());

    for (design, redline) in [(porsche, 9000.0), (turbo, 7000.0)] {
        for rpm in [0.0, 4000.0, redline] {
            let t = Telemetry { rpm, redline_rpm: redline, limiter_rpm: redline, gear: 2, speed_kmh: 88.0, boost_bar: 1.2, limiter: rpm >= redline, ..Default::default() };
            let o = render(&design, &t, 1280, 720);
            assert!(o.width > 0 && o.height > 0 && o.x + o.width <= 1280 && o.y + o.height <= 720, "{}", design.name);
            assert!(o.rgba.iter().skip(3).step_by(4).filter(|a| **a > 0).count() > (o.width * o.height / 10) as usize);
        }
    }
}
