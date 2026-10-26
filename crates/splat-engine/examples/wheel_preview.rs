//! Offscreen visual inspection with the real car, real rig and production shaders.
//! wheel_preview <output-directory> [frame-count] [speed-kmh] [steering-degrees]
use splat_engine::drive::WheelVisualState;
use splat_engine::{CarLoad, Engine, EngineConfig, Level, Logger};
use splat_render::{camera, scene, sog};
use std::{path::Path, sync::Arc};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let output = Path::new(args.first().expect("output directory"));
    std::fs::create_dir_all(output).unwrap();
    let frames: usize = args.get(1).map_or(1, |s| s.parse().unwrap());
    let speed: f64 = args.get(2).map_or(20.0, |s| s.parse().unwrap());
    let steering: f32 = args
        .get(3)
        .map_or(0.0, |s| s.parse::<f32>().unwrap().to_radians());
    let bundle = sog::load(Path::new("assets/car/porsche-992-gt3rs-2023.sog")).unwrap();
    let config = EngineConfig::parse(&std::fs::read_to_string("engine.conf").unwrap()).unwrap();
    let log = Arc::new(Logger::open(None, Level::Warn, 100).unwrap());
    let mut engine = Engine::new(
        config,
        None,
        log,
        CarLoad::Loaded(sog::centres(&bundle)),
        None,
    );
    let rig = splat_engine::wheel_visual::WheelRig::fit(&engine, &bundle).unwrap();
    println!(
        "Part counts {:?}; body hubs {:?}",
        rig.counts(),
        rig.body_hubs
    );
    let (device, queue) = splat_render::try_device().unwrap();
    let pipelines = scene::Pipelines::new(&device, splat_render::COLOR_FORMAT).unwrap();
    let mut car = scene::build(&device, &queue, &pipelines, &bundle, 0.5).unwrap();
    car.configure_wheels(&device, &queue, &rig.parts);
    let close = std::env::var("CLOSE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|i| *i < 4);
    let center = close.map_or_else(
        || camera::transform_point(&engine.model, sog::mean_centre(&sog::centres(&bundle))),
        |i| camera::transform_point(&engine.model, rig.hubs[i]),
    );
    let side = std::env::var("SIDE")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(1.0);
    let view = scene::View {
        model: engine.model,
        view: camera::look_at(
            [
                center[0] + side * if close.is_some() { 1.9 } else { 5.5 },
                center[1] + if close.is_some() { 0.03 } else { 0.7 },
                center[2] + if close.is_some() { 0.06 } else { 0.4 },
            ],
            center,
            [0.0, 1.0, 0.0],
        ),
        projection: camera::perspective(42f32.to_radians(), 1600.0 / 900.0, 0.01, 100.0),
        width: 1600,
        height: 900,
        near: 0.01,
        far: 100.0,
    };
    let accel60 = std::env::var("ACCEL60").is_ok();
    let simulated = accel60 || std::env::var("SIM").is_ok();
    let shutter_samples: usize = std::env::var("SHUTTER_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(if accel60 { 4 } else { 1 })
        .clamp(1, 16);
    let mut held: Option<[WheelVisualState; 4]> = None;
    let mut mph = 0.0f32;
    let mut trace = String::from("frame,time_s,mph,rpm,gear\n");
    let hud =
        accel60.then(|| splat_hud::HudDesign::parse(include_str!("wheel_preview.hud")).unwrap());
    if simulated {
        for _ in 0..120 {
            engine.tick();
        }
        engine.set_controls(splat_car::Controls {
            throttle: if accel60 { 0.0 } else { 1.0 },
            steer: steering.sin(),
            ..Default::default()
        });
    }
    let phase = std::env::var("PHASE_DEG")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
        .to_radians();
    for index in 0..frames {
        let time = index as f64 / 30.0;
        let mut states = if simulated {
            if let Some(mut cruise) = held {
                for wheel in &mut cruise {
                    wheel.spin += f64::from(wheel.angular_speed) / 30.0;
                }
                held = Some(cruise);
                cruise
            } else {
                if accel60 && index == 15 {
                    engine.set_controls(splat_car::Controls {
                        throttle: 1.0,
                        ..Default::default()
                    });
                }
                for _ in 0..2 {
                    engine.tick();
                }
                engine.set_render_alpha(1.0);
                if index % 30 == 0 {
                    println!("SIM {}", engine.status_json()["car"]);
                }
                let mut wheels = engine.render_wheels();
                if accel60 {
                    mph = engine.physics.car_output().unwrap().speed_mps.abs() / 0.44704;
                    if mph >= 60.0 {
                        let scale = 60.0 / mph;
                        for wheel in &mut wheels {
                            wheel.angular_speed *= scale;
                        }
                        mph = 60.0;
                        held = Some(wheels);
                        println!(
                            "Reached 60 mph at {:.3}s after throttle",
                            (index - 15) as f64 / 30.0
                        );
                    }
                }
                wheels
            }
        } else {
            std::array::from_fn(|i| WheelVisualState {
                spin: phase + time * (speed / 3.6) / if i < 2 { 0.350 } else { 0.367 },
                angular_speed: ((speed / 3.6) / if i < 2 { 0.350 } else { 0.367 }) as f32,
                steering: if i < 2 { steering } else { 0.0 },
                grounded: true,
                ..Default::default()
            })
        };
        if std::env::var("MOTION").as_deref() == Ok("0") {
            states.iter_mut().for_each(|state| {
                state.spin = 0.0;
                state.angular_speed = 0.0;
            });
        }
        let mut rgba = if shutter_samples == 1 {
            car.set_wheel_animation(&queue, &rig.animation(states, false));
            car.render_rgba(&device, &queue, &pipelines, &view, None)
        } else {
            // Film preview only: average exposures of the continuously rotating wheel shader.
            // Blur adds no extra Gaussian stretch or depth displacement.
            let linear: Vec<u32> = (0..256)
                .map(|i| {
                    let s = i as f32 / 255.0;
                    let l = if s <= 0.04045 {
                        s / 12.92
                    } else {
                        ((s + 0.055) / 1.055).powf(2.4)
                    };
                    (l * 65535.0).round() as u32
                })
                .collect();
            let mut sum = vec![0u32; 1600 * 900 * 4];
            for sample in 0..shutter_samples {
                let dt = ((sample as f64 + 0.5) / shutter_samples as f64 - 0.5) / 30.0;
                let exposure = states.map(|s| WheelVisualState {
                    spin: s.spin + f64::from(s.angular_speed) * dt,
                    ..s
                });
                car.set_wheel_animation(&queue, &rig.animation(exposure, false));
                let frame = car.render_rgba(&device, &queue, &pipelines, &view, None);
                for (i, value) in frame.iter().enumerate() {
                    sum[i] += if i % 4 == 3 {
                        u32::from(*value) * 257
                    } else {
                        linear[*value as usize]
                    };
                }
            }
            let srgb: Vec<u8> = (0..65536)
                .map(|i| {
                    let l = i as f32 / 65535.0;
                    let s = if l <= 0.0031308 {
                        l * 12.92
                    } else {
                        1.055 * l.powf(1.0 / 2.4) - 0.055
                    };
                    (s * 255.0).round().clamp(0.0, 255.0) as u8
                })
                .collect();
            sum.iter()
                .enumerate()
                .map(|(i, s)| {
                    let v = *s / shutter_samples as u32;
                    if i % 4 == 3 {
                        (v / 257) as u8
                    } else {
                        srgb[v as usize]
                    }
                })
                .collect()
        };
        if let Some(hud) = &hud {
            let mut telemetry = splat_engine::render::hud_telemetry(&engine).unwrap();
            // This preview design labels its speed channel MPH, so supply the converted display value.
            telemetry.speed_kmh = mph;
            let overlay = splat_hud::render_at_time(hud, &telemetry, 1600, 900, time as f32);
            for y in 0..overlay.height {
                for x in 0..overlay.width {
                    let src = ((y * overlay.width + x) * 4) as usize;
                    let dst = (((overlay.y + y) * 1600 + overlay.x + x) * 4) as usize;
                    let alpha = u32::from(overlay.rgba[src + 3]);
                    for c in 0..3 {
                        rgba[dst + c] = (u32::from(overlay.rgba[src + c])
                            + u32::from(rgba[dst + c]) * (255 - alpha) / 255)
                            .min(255) as u8;
                    }
                    rgba[dst + 3] = 255;
                }
            }
            trace.push_str(&format!(
                "{index},{time:.6},{mph:.6},{:.1},{}\n",
                telemetry.rpm, telemetry.gear
            ));
        }
        // The engine has no image dependency, so write a lossless PAM that standard image tools can read.
        let path = output.join(format!("frame-{index:03}.pam"));
        let mut bytes = format!(
            "P7\nWIDTH 1600\nHEIGHT 900\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
        )
        .into_bytes();
        bytes.extend(rgba);
        std::fs::write(path, bytes).unwrap();
        if index == 0 {
            car.set_wheel_animation(&queue, &rig.animation(states, true));
            let rgba = car.render_rgba(&device, &queue, &pipelines, &view, None);
            let mut bytes =
                b"P7\nWIDTH 1600\nHEIGHT 900\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
                    .to_vec();
            bytes.extend(rgba);
            std::fs::write(output.join("selection.pam"), bytes).unwrap();
        }
    }
    if accel60 {
        std::fs::write(output.join("telemetry.csv"), trace).unwrap();
    }
    println!("Wrote {frames} frames to {}", output.display());
}
