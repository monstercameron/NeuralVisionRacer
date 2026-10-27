//! CPU-only streaming selection probe: no graphics adapter or device is created.
//!
//! cargo run --release --example lod_selection_probe -- <level-dir> [samples=4096] [rounds=4] [reverse]
//!
//! The inputs are precomputed and use a real level tree. Timings cover only settings updates and `select`, after a
//! warmup; fingerprints are separate, untimed replays of every leaf, stats, focus distance and request order. The
//! public CPU-only path leaves all ranges nonresident; resident/keep-delay behavior is covered by private-state tests.

use splat_render::{camera, lod::LodMeta, stream::{LevelStream, StreamSettings}};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

struct CountingAllocator;
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout)
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.realloc(pointer, layout, size)
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy)]
struct Sample {
    eye: [f32; 3],
    forward: [f32; 3],
    clip: Option<[f32; 16]>,
    capacity: u32,
    max_distance_m: f32,
    lod_error_px: f32,
    focal_px: f32,
}

fn inputs(meta: &LodMeta, case: &str) -> Vec<Sample> {
    let projection = camera::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.05, 2000.0);
    (0..360).map(|frame| {
        let t = frame as f32 / 60.0;
        let mut eye = [-12.0 + 4.0 * t, -2.5, -7.0 + 2.0 * (t * 0.4).sin()];
        let mut angle = 0.2 * (t * 0.7).sin();
        let mut capacity = 3_000_000;
        let mut max_distance_m = 150.0;
        let mut lod_error_px = 1.5;
        let mut focal_px = projection[5] * 540.0;
        let mut with_clip = true;
        match case {
            "drive-screen" => {}
            "band-edges" => {
                let leaf = &meta.leaves[(frame / 12) % meta.leaves.len()];
                let distance = [0.0, 7.999, 8.0, 8.001, 23.999, 24.0, 24.001, 47.999, 48.0, 48.001, 149.999, 150.001][frame % 12];
                eye = [leaf.min[0] - distance, (leaf.min[1] + leaf.max[1]) * 0.5, (leaf.min[2] + leaf.max[2]) * 0.5];
                lod_error_px = 0.0;
            }
            "budget-frustum" => {
                capacity = [180_000, 500_000, 1_000_000, 3_000_000][(frame / 45) % 4];
                max_distance_m = [80.0, 150.0, 240.0][(frame / 60) % 3];
                angle += std::f32::consts::PI * (frame as f32 / 90.0);
                focal_px = [600.0, 935.3074, 1400.0][(frame / 30) % 3];
            }
            "no-frustum" => {
                with_clip = false;
                angle = std::f32::consts::PI * (frame as f32 / 180.0);
                lod_error_px = if frame % 120 < 60 { 0.0 } else { 1.5 };
            }
            _ => unreachable!(),
        }
        let forward = [angle.cos(), 0.0, angle.sin()];
        let target = [eye[0] + forward[0], eye[1], eye[2] + forward[2]];
        let view = camera::look_at(eye, target, [0.0, 1.0, 0.0]);
        Sample { eye, forward, clip: with_clip.then(|| camera::mul(&projection, &view)), capacity, max_distance_m, lod_error_px, focal_px }
    }).collect()
}

fn update(stream: &mut LevelStream, sample: &Sample, frame: usize) {
    stream.settings.capacity = sample.capacity;
    stream.settings.max_distance_m = sample.max_distance_m;
    stream.settings.lod_error_px = sample.lod_error_px;
    stream.set_focal_px(sample.focal_px);
    stream.select(sample.eye, sample.forward, sample.clip.as_ref(), frame as f32 / 60.0);
}

fn fingerprint(stream: &mut LevelStream, samples: &[Sample]) -> u64 {
    let mut hash = 14_695_981_039_346_656_037u64;
    let mut write = |bytes: &[u8]| {
        for &byte in bytes {
            hash = (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211);
        }
    };
    for (frame, sample) in samples.iter().enumerate() {
        update(stream, sample, frame);
        write(stream.nearest_leaves_json(stream.lod_meta().leaves.len()).as_bytes());
        write(stream.stats().json().to_string().as_bytes());
        write(&stream.sharp_distance().to_bits().to_le_bytes());
        // A failed request becomes eligible on the next frame. This exercises full request order, not network I/O.
        for (file, folder) in stream.requests() {
            write(&file.to_le_bytes());
            write(folder.as_bytes());
            stream.failed(file);
        }
    }
    hash
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().expect("level directory"));
    let samples: usize = args.get(1).map_or(4096, |s| s.parse().expect("sample count"));
    let rounds: usize = args.get(2).map_or(4, |s| s.parse().expect("round count"));
    let mut cases = ["drive-screen", "band-edges", "budget-frustum", "no-frustum"];
    if args.get(3).is_some_and(|s| s == "reverse") { cases.reverse(); }
    assert!(samples > 0 && rounds > 0);
    let meta = LodMeta::parse(&std::fs::read(dir.join("lod-meta.json")).expect("lod-meta.json")).expect("level tree");
    let metas: Vec<Option<Vec<u8>>> = meta.filenames.iter().map(|file| std::fs::read(dir.join(file)).ok()).collect();
    let make_stream = || LevelStream::new(meta.clone(), &metas, StreamSettings::default()).expect("stream");
    for case in cases {
        let inputs = inputs(&meta, case);
        let fingerprint = fingerprint(&mut make_stream(), &inputs);
        for round in 0..rounds {
            let mut stream = make_stream();
            for frame in 0..360 { update(&mut stream, &inputs[frame % inputs.len()], frame); }
            let before = ALLOCATIONS.load(Ordering::Relaxed);
            let start = Instant::now();
            for frame in 0..samples {
                update(std::hint::black_box(&mut stream), std::hint::black_box(&inputs[frame % inputs.len()]), frame + 360);
            }
            let elapsed = start.elapsed().as_secs_f64() * 1e9 / samples as f64;
            let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
            std::hint::black_box(&stream);
            println!("{{\"case\":\"{case}\",\"round\":{round},\"samples\":{samples},\"leaves\":{},\"ns_per_select\":{elapsed},\"allocations\":{allocations},\"fingerprint\":\"{fingerprint:016x}\"}}", meta.leaves.len());
        }
    }
}
