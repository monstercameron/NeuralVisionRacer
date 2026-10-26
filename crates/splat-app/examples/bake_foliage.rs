//! Bake conservative 3D foliage material cells from source SOG colour and collision geometry.
//! Run: bake_foliage <output.json> [level ...]. Original scan assets remain unchanged.
use splat_engine::{
    foliage::{Evidence, CELL_M},
    LevelFloor,
};
use splat_render::{lod::LodMeta, sog};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
fn key(p: [f32; 3]) -> [i32; 3] {
    p.map(|v| (v / CELL_M).floor() as i32)
}
fn main() {
    let a: Vec<_> = std::env::args().skip(1).collect();
    let names: Vec<_> = if a.len() > 1 {
        a[1..].iter().map(String::as_str).collect()
    } else {
        vec![
            "trompetstraat-delft",
            "nmbu-park-03",
            "basilica-assumption",
            "saburi-alley-yazd",
        ]
    };
    let overrides: serde_json::Value =
        serde_json::from_slice(&std::fs::read("specs/foliage-overrides.json").unwrap()).unwrap();
    let mut levels = serde_json::Map::new();
    for name in names {
        let dir = Path::new("levels").join(name);
        let floor = LevelFloor::load(&dir, 2.0).unwrap();
        let mut ground = BTreeMap::<[i32; 2], f32>::new();
        for c in &floor.cells {
            let p = floor.top_of(*c);
            let k = [p[0].floor() as i32, p[2].floor() as i32];
            ground
                .entry(k)
                .and_modify(|y| *y = y.min(p[1]))
                .or_insert(p[1]);
        }
        let above_ground = |p: [f32; 3]| {
            let k = [p[0].floor() as i32, p[2].floor() as i32];
            let mut y = f32::INFINITY;
            for dx in -1..=1 {
                for dz in -1..=1 {
                    if let Some(v) = ground.get(&[k[0] + dx, k[1] + dz]) {
                        y = y.min(*v);
                    }
                }
            }
            y.is_finite() && p[1] - y > 0.30
        };
        // Restrict source-colour work/output to material cells touching actual obstacle/floor geometry.
        let mut wanted = BTreeSet::new();
        for b in floor.wall_boxes.as_ref().unwrap() {
            let lo = key([0, 1, 2].map(|i| b.min[i] + floor.origin[i]));
            let hi = key([0, 1, 2].map(|i| b.max[i] + floor.origin[i] - 1e-4));
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        wanted.insert([x, y, z]);
                    }
                }
            }
        }
        for c in &floor.cells {
            wanted.insert(key(floor.top_of(*c)));
        }
        let meta = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).unwrap()).unwrap();
        let lod = meta.pick(-1, 3_000_000);
        let mut votes = BTreeMap::<[i32; 3], Evidence>::new();
        for chunk in meta.chunk_dirs(lod) {
            let b = sog::load_dir(&dir.join(chunk)).unwrap();
            let colours = &b.textures["sh0"].rgba;
            let scales = &b.textures["scales"].rgba;
            for (i, p) in sog::centres(&b).iter().enumerate() {
                let p = [-p[0], -p[1], p[2]];
                let k = key(p);
                if !wanted.contains(&k) || colours[i * 4 + 3] < 170 || !above_ground(p) {
                    continue;
                }
                if (0..3)
                    .any(|axis| b.meta.scales_codebook[scales[i * 4 + axis] as usize].exp() > 0.3)
                {
                    continue;
                }
                let rgb = [0, 1, 2].map(|axis| {
                    (0.5 + 0.2820948 * b.meta.sh0_codebook[colours[i * 4 + axis] as usize]).max(0.0)
                });
                votes.entry(k).or_default().push(p, rgb);
            }
        }
        if let Ok(path) = std::env::var("EVIDENCE_DIR") {
            let stats:Vec<_>=votes.iter().map(|(k,v)|serde_json::json!({"cell":k,"thickness_m":v.thickness(),"foliage":v.foliage()})).collect();
            std::fs::write(
                Path::new(&path).join(format!("{name}.json")),
                serde_json::to_vec(&stats).unwrap(),
            )
            .unwrap();
        }
        // Require local evidence and spatial support; no dilation into adjacent masonry/trunks.
        let protected = |k: &&[i32; 3]| {
            overrides["levels"][name]["hard_regions"]
                .as_array()
                .is_some_and(|regions| {
                    regions.iter().any(|r| {
                        (0..3).all(|i| {
                            ((k[i] + 1) as f32 * CELL_M) > r["min"][i].as_f64().unwrap() as f32
                                && (k[i] as f32 * CELL_M) < r["max"][i].as_f64().unwrap() as f32
                        })
                    })
                })
        };
        let candidates: BTreeSet<_> = votes
            .iter()
            .filter(|(k, v)| v.foliage() && !protected(k))
            .map(|(k, _)| *k)
            .collect();
        let cells: Vec<_> = candidates
            .iter()
            .filter(|k| {
                let mut neighbours = 0;
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for dz in -1..=1 {
                            if dx == 0 && dy == 0 && dz == 0 {
                                continue;
                            }
                            neighbours += usize::from(candidates.contains(&[
                                k[0] + dx,
                                k[1] + dy,
                                k[2] + dz,
                            ]));
                        }
                    }
                }
                neighbours >= 2
            })
            .copied()
            .collect();
        println!("{name}: {} material cells, LOD {lod}", cells.len());
        levels.insert(name.into(),serde_json::json!({"source_lod":lod,"cells":cells,"classifier":"opaque, compact source splats; >=6 green votes and >=60% green, >1.4cm PCA thickness, >=2 neighbouring candidates; >0.3 m above nearby floor; reviewed hard-region overrides; unknown material remains solid"}));
    }
    std::fs::write(
        &a[0],
        serde_json::to_vec(&serde_json::json!({"cell_m":CELL_M,"levels":levels})).unwrap(),
    )
    .unwrap();
}
