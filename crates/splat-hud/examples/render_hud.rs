//! Renders a HUD design to a PNG, composited over a mid-grey 1280×720 screen, and prints the render time.
//!
//! `cargo run --release --example render_hud -- <design name or path> <out.png> [rpm speed gear boost]`
//!
//! The screen size can be changed with `HUD_SCREEN=1920x1080`. Other telemetry is fixed at warm, healthy values;
//! rpm at or above the car's limiter sets `limiter`.

use std::time::Instant;

use splat_hud::{render, HudDesign, Telemetry};
use tiny_skia::{IntSize, Pixmap, PixmapPaint, Transform};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: render_hud <design name or path> <out.png> [rpm speed gear boost]");
        std::process::exit(2);
    }
    let design = HudDesign::by_name(&args[0]).unwrap_or_else(|| {
        let text = std::fs::read_to_string(&args[0]).unwrap_or_else(|e| fail(&format!("{}: {e}", args[0])));
        HudDesign::parse(&text).unwrap_or_else(|e| fail(&format!("{}: {e}", args[0])))
    });
    let number = |i: usize, default: f32| args.get(i).map_or(default, |v| v.parse().unwrap_or_else(|_| fail(&format!("`{v}` is not a number"))));

    // The GT3 RS revs to 9,000; the generic turbo car is given a 7,000 rpm redline.
    let (redline, limiter) = if design.name.contains("GT3") { (9000.0, 9000.0) } else { (7000.0, 7200.0) };
    let rpm = number(2, 6500.0);
    let telemetry = Telemetry {
        rpm,
        speed_kmh: number(3, 142.0),
        gear: number(4, 3.0) as i32,
        boost_bar: number(5, 0.0),
        redline_rpm: redline,
        limiter_rpm: limiter,
        throttle: 0.85,
        brake: 0.0,
        water_temp_c: 92.0,
        oil_temp_c: 108.0,
        oil_pressure_bar: 4.6,
        limiter: rpm >= limiter,
        launch: false,
        wheelspin: false,
    };
    let (sw, sh) = std::env::var("HUD_SCREEN")
        .ok()
        .and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))))
        .unwrap_or((1280u32, 720u32));

    // Time it: warm up, then take the mean and best of many frames.
    for _ in 0..20 {
        render(&design, &telemetry, sw, sh);
    }
    let runs = 300;
    let mut best = f64::MAX;
    let start = Instant::now();
    for i in 0..runs {
        // Vary rpm a little so nothing can be cached between frames.
        let t = Telemetry { rpm: telemetry.rpm + (i % 7) as f32, ..telemetry };
        let s = Instant::now();
        std::hint::black_box(render(&design, &t, sw, sh));
        best = best.min(s.elapsed().as_secs_f64());
    }
    let mean = start.elapsed().as_secs_f64() / runs as f64;
    let overlay = render(&design, &telemetry, sw, sh);
    println!(
        "{}: {}x{} overlay at ({}, {}) on a {sw}x{sh} screen; render mean {:.3} ms, best {:.3} ms",
        design.name,
        overlay.width,
        overlay.height,
        overlay.x,
        overlay.y,
        mean * 1e3,
        best * 1e3
    );

    let mut screen = Pixmap::new(sw, sh).expect("screen size");
    screen.fill(tiny_skia::Color::from_rgba8(128, 128, 128, 255));
    if overlay.width > 0 && overlay.height > 0 {
        let size = IntSize::from_wh(overlay.width, overlay.height).expect("overlay size");
        let hud = Pixmap::from_vec(overlay.rgba, size).expect("overlay pixels");
        screen.draw_pixmap(overlay.x as i32, overlay.y as i32, hud.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    screen.save_png(&args[1]).unwrap_or_else(|e| fail(&format!("{}: {e}", args[1])));
    println!("wrote {}", args[1]);
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}
