//! Review the actual classified source collision against the captured scan.
use splat_engine::{collision::LevelCollision, EngineConfig, LevelFloor};
use splat_render::{camera, frame::Frame, lod::LodMeta, scene::View, sog};
use std::{fs, path::Path};
fn main() {
    let a: Vec<_> = std::env::args().skip(1).collect();
    let name = &a[0];
    let out = Path::new(&a[1]);
    fs::create_dir_all(out).unwrap();
    let mut cfg = EngineConfig::parse(&fs::read_to_string("engine.conf").unwrap()).unwrap();
    cfg.level_path = format!("levels/{name}");
    let dir = Path::new(&cfg.level_path);
    let level = LevelFloor::load(dir, cfg.level_floor_clearance_m).unwrap();
    let (eye, target) = if name == "trompetstraat-delft" {
        ([1.5, 2.5, 25.5], [-2.5, -0.1, 30.5])
    } else {
        ([21.0, -2.0, -64.0], [24.5, -6.0, -57.5])
    };
    let parse = |key: &str, default: [f32; 3]| {
        std::env::var(key)
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|v| v.parse::<f32>().unwrap())
                    .collect::<Vec<_>>()
            })
            .map_or(default, |v| [v[0], v[1], v[2]])
    };
    let eye = parse("EYE", eye);
    let target = parse("TARGET", target);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut frame =
        pollster::block_on(Frame::new(&instance, None, (1100, 760), false, None)).unwrap();
    let meta = LodMeta::parse(&fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
    let lod = meta.pick(-1, 3_000_000);
    let chunks: Vec<_> = meta
        .chunk_dirs(lod)
        .iter()
        .map(|c| sog::load_dir(&dir.join(c)).unwrap())
        .collect();
    frame.set_level(&chunks).unwrap();
    frame.set_level_model([
        -1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]);
    let view = View {
        model: camera::IDENTITY,
        view: camera::look_at(eye, target, [0.0, 1.0, 0.0]),
        projection: camera::perspective(60f32.to_radians(), 1100.0 / 760.0, 0.03, 1000.0),
        width: 1100,
        height: 760,
        near: 0.03,
        far: 1000.0,
    };
    let save = |frame: &mut Frame, label: &str| {
        image::RgbaImage::from_raw(1100, 760, pollster::block_on(frame.read_rgba(&view)))
            .unwrap()
            .save(out.join(format!("{name}-{label}.png")))
            .unwrap()
    };
    save(&mut frame, "scan");
    let mut report = serde_json::json!({"level":name,"eye":eye,"target":target,"lod":lod});
    for (i, soft) in [false, true].into_iter().enumerate() {
        cfg.level_foliage = soft;
        let mut collision = LevelCollision::new(&level, &cfg);
        let mut physics = splat_engine::Physics::new(&cfg);
        let world = &mut physics.world;
        collision.update(world, target);
        world.step();
        let lines = collision
            .debug_material_segments(world, 4)
            .into_iter()
            .filter(|(a, b, m)| {
                *m != 0
                    && [a, b].iter().all(|p| {
                        (p[0] - target[0]).hypot(p[2] - target[2]) < 5.0
                            && (p[1] - target[1]).abs() < 3.0
                    })
            })
            .map(|(a, b, m)| {
                (
                    a,
                    b,
                    if m == 2 {
                        [0.15, 1.0, 0.25, 0.65]
                    } else {
                        [1.0, 0.1, 0.7, 0.28]
                    },
                )
            })
            .collect();
        frame.set_world_debug(Some(i as u64), || lines);
        let label = if soft { "after" } else { "before" };
        save(&mut frame, label);
        report[label] = collision.foliage_profile();
    }
    fs::write(
        out.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("{report}");
}
