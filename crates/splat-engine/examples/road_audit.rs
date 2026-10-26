//! Export source floor cells and fitted obstacle boxes for a whole-road collision audit.
use splat_engine::{EngineConfig, LevelFloor};
use std::{io::Write, path::Path};
fn main() {
    let a: Vec<_> = std::env::args().skip(1).collect();
    let dir = Path::new(&a[0]);
    let out = Path::new(&a[1]);
    std::fs::create_dir_all(out).unwrap();
    let l = LevelFloor::load(dir, EngineConfig::default().level_floor_clearance_m).unwrap();
    let mut f = std::fs::File::create(out.join("floor.bin")).unwrap();
    for c in &l.cells {
        for v in l.top_of(*c) {
            f.write_all(&v.to_le_bytes()).unwrap();
        }
    }
    let mut f = std::fs::File::create(out.join("walls.bin")).unwrap();
    for b in l.wall_boxes.as_ref().unwrap() {
        for v in b.min.into_iter().chain(b.max) {
            f.write_all(&v.to_le_bytes()).unwrap();
        }
    }
    std::fs::write(out.join("meta.json"),serde_json::to_vec_pretty(&serde_json::json!({"origin":l.origin,"dims":l.dims,"resolution":l.resolution,"cells":l.cells.len(),"boxes":l.wall_boxes.as_ref().unwrap().len()})).unwrap()).unwrap();
}
