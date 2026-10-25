//! Prints what the level's collision voxels contain: occupancy, floor cells, and where the floor sits in height.
//! `cargo run --release --example probe_level -- <level dir>`
use splat_engine::svo::{grid_cells, Octree};
use std::path::Path;
use std::time::Instant;

fn main() {
    let dir = std::env::args().nth(1).expect("level directory");
    let started = Instant::now();
    let tree = Octree::load(Path::new(&dir)).expect("load voxel collision");
    println!("loaded in {:?}; grid {:?} cells, min {:?}, res {}", started.elapsed(), grid_cells(&tree.meta), tree.meta.grid_min, tree.meta.resolution);

    let started = Instant::now();
    let mut occupied = 0u64;
    let mut y_hist = vec![0u64; grid_cells(&tree.meta)[1] as usize];
    tree.for_each_occupied(|c| {
        occupied += 1;
        y_hist[c[1] as usize] += 1;
    });
    println!("occupied cells {occupied} in {:?}", started.elapsed());
    let lowest = y_hist.iter().position(|&n| n > 0).unwrap_or(0);
    let highest = y_hist.iter().rposition(|&n| n > 0).unwrap_or(0);
    println!("occupied y range: cell {lowest}..{highest} = metres {:.2}..{:.2}", tree.meta.grid_min[1] + lowest as f32 * tree.meta.resolution, tree.meta.grid_min[1] + highest as f32 * tree.meta.resolution);

    if let Some(out) = std::env::args().nth(2) {
        write_height_map(&tree, 40, &out);
    }
    for clearance in [20u32, 40] {
        let started = Instant::now();
        let floor = tree.floor_cells(clearance);
        let mut hist = std::collections::BTreeMap::new();
        for c in &floor {
            *hist.entry(c[1]).or_insert(0u64) += 1;
        }
        let top = hist.iter().max_by_key(|(_, n)| **n).map(|(y, n)| (*y, *n)).unwrap_or((0, 0));
        println!("clearance {clearance} cells ({:.2} m): {} floor columns in {:?}; most common floor y = cell {} ({:.2} m, {} columns)", clearance as f32 * tree.meta.resolution, floor.len(), started.elapsed(), top.0, tree.meta.grid_min[1] + top.0 as f32 * tree.meta.resolution, top.1);
    }
}

/// Writes a grey PGM of the lowest walkable height per column (`clearance` cells of air above), downsampled by 2.
fn write_height_map(tree: &Octree, clearance: u32, path: &str) {
    let dims = grid_cells(&tree.meta);
    let floor = tree.floor_cells(clearance);
    let (w, h) = ((dims[0] / 2) as usize, (dims[2] / 2) as usize);
    let mut pixels = vec![0u8; w * h];
    for c in floor {
        let (x, z) = ((c[0] / 2) as usize, (c[2] / 2) as usize);
        if x < w && z < h {
            let value = 40 + (c[1] as f32 / dims[1] as f32 * 215.0) as u8;
            let slot = &mut pixels[z * w + x];
            *slot = (*slot).max(value);
        }
    }
    let mut out = format!("P5\n{w} {h}\n255\n").into_bytes();
    out.extend_from_slice(&pixels);
    std::fs::write(path, out).expect("write map");
    println!("wrote {path} ({w}x{h})");
}
