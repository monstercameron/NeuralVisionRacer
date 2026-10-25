//! Prints a full-throttle run from rest for a car spec: 0-100, 0-200, the quarter mile, top speed and every shift.
//! `cargo run --example accel [-- path/to/car.car]` (the GT3 RS by default)
use splat_car::{straight_line, CarSpec};

fn main() {
    let spec = match std::env::args().nth(1) {
        Some(path) => CarSpec::parse(&std::fs::read_to_string(&path).expect("read spec")).expect("parse spec"),
        None => CarSpec::porsche_992_gt3_rs(),
    };
    let (kw, rpm) = spec.peak_power();
    println!("{}: {:.0} kW ({:.0} hp) at {rpm:.0} rpm, {:.0} Nm peak, {} kg", spec.name, kw, kw * 1.341, spec.torque_curve.iter().map(|p| p.1).fold(0.0, f32::max), spec.mass_kg);
    let samples = straight_line(&spec, 120.0, 1.0 / 240.0);
    let time_to = |kmh: f32| samples.iter().find(|s| s.1 * 3.6 >= kmh).map(|s| s.0);
    for kmh in [100.0, 200.0, 250.0] {
        match time_to(kmh) {
            Some(t) => println!("0-{kmh:.0} km/h: {t:.2} s"),
            None => println!("0-{kmh:.0} km/h: not reached"),
        }
    }
    let mut distance = 0.0;
    for s in &samples {
        distance += s.1 / 240.0;
        if distance >= 402.336 {
            println!("quarter mile: {:.2} s at {:.0} km/h", s.0, s.1 * 3.6);
            break;
        }
    }
    let top = samples.iter().map(|s| s.1).fold(0.0, f32::max);
    let last = samples.last().unwrap().2;
    println!("top speed: {:.1} km/h in gear {} at {:.0} rpm", top * 3.6, last.gear, last.rpm);
    for pair in samples.windows(2) {
        if pair[1].2.gear != pair[0].2.gear {
            println!("  shift {} -> {} at {:.2} s, {:.0} km/h, {:.0} rpm", pair[0].2.gear, pair[1].2.gear, pair[1].0, pair[1].1 * 3.6, pair[0].2.rpm);
        }
    }
}
