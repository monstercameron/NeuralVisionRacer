//! Prints where each chunk of a level's LOD lies: percentiles of its splat centres per axis.
//! `cargo run --release --example chunk_stats -- <level dir> <lod>`
use splat_render::{lod::LodMeta, sog};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(&args[0]);
    let lod: i32 = args[1].parse().unwrap();
    let meta = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
    for chunk in meta.chunk_dirs(meta.pick(lod, 0)) {
        let bundle = sog::load_dir(&dir.join(&chunk)).unwrap();
        let centres = sog::centres(&bundle);
        let pct = |axis: usize| {
            let mut v: Vec<f32> = centres.iter().map(|c| c[axis]).collect();
            v.sort_by(f32::total_cmp);
            [0.05, 0.25, 0.5, 0.75, 0.95].map(|q| v[((v.len() - 1) as f32 * q) as usize])
        };
        println!("{chunk}: n={} mean={:?}", centres.len(), sog::mean_centre(&centres).map(|v| v.round()));
        for (name, axis) in [("x", 0), ("y", 1), ("z", 2)] {
            println!("   {name} 5/25/50/75/95%: {:?}", pct(axis).map(|v| (v * 10.0).round() / 10.0));
        }
    }
}
