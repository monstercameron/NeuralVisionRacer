//! CPU-only centre serialization probe. No graphics adapter or device is created.
//!
//! cargo run --release --example centre_bytes_probe -- <unpacked-SOG-chunk-dir> [rounds=4] [reverse]
//!
//! Uses decoded centres from the supplied real chunk, with a few signed-zero, infinity, subnormal, and NaN payload
//! fixtures added. The pool case uses the actual NaN initialization pattern. Timings cover only conversion to bytes,
//! not GPU queue staging or rendering; fingerprints and fallback checks are computed outside the timed loops. The
//! separate release cases compare the current two allocations with a possible single-allocation NaN byte buffer.
//! Pass zero rounds to check fingerprints and allocation counts without taking timings.

use splat_render::sog;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    borrow::Cow,
    hint::black_box,
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

fn original(centres: &[[f32; 3]]) -> Vec<u8> {
    centres.concat().iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn fallback(centres: &[[f32; 3]]) -> Vec<u8> {
    centres.iter().flat_map(|centre| centre.iter().flat_map(|value| value.to_le_bytes())).collect()
}

fn byte_view(centres: &[[f32; 3]]) -> Cow<'_, [u8]> {
    #[cfg(target_endian = "little")]
    { Cow::Borrowed(bytemuck::cast_slice(centres)) }
    #[cfg(target_endian = "big")]
    { Cow::Owned(fallback(centres)) }
}

fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(14_695_981_039_346_656_037u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211)
    })
}

fn time(centres: &[[f32; 3]], repetitions: usize, view: bool) -> (f64, f64) {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let start = Instant::now();
    for _ in 0..repetitions {
        if view {
            black_box(byte_view(black_box(centres)));
        } else {
            black_box(original(black_box(centres)));
        }
    }
    let ns = start.elapsed().as_secs_f64() * 1e9 / repetitions as f64;
    let allocations = (ALLOCATIONS.load(Ordering::Relaxed) - before) as f64 / repetitions as f64;
    (ns, allocations)
}

fn release_original(count: usize) -> Vec<u8> {
    vec![f32::NAN; count * 3].iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn release_single_allocation(count: usize) -> Vec<u8> {
    f32::NAN.to_le_bytes().repeat(count * 3)
}

fn time_release(count: usize, repetitions: usize, single_allocation: bool) -> (f64, f64) {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let start = Instant::now();
    for _ in 0..repetitions {
        if single_allocation {
            black_box(release_single_allocation(black_box(count)));
        } else {
            black_box(release_original(black_box(count)));
        }
    }
    let ns = start.elapsed().as_secs_f64() * 1e9 / repetitions as f64;
    let allocations = (ALLOCATIONS.load(Ordering::Relaxed) - before) as f64 / repetitions as f64;
    (ns, allocations)
}

// The empty pool contains only the same NaN. This fixture measures whether its otherwise-unused initial centre and
// bounds can be derived from that known value rather than scanning and selecting over the whole pool. The production
// path is unchanged; this is an exploratory CPU measurement.
fn pool_geometry_reference(centres: &[[f32; 3]]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let centre = sog::mean_centre(centres);
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    if !centres.is_empty() {
        let mut values = vec![0.0f32; centres.len()];
        let (a, b) = (centres.len() / 200, centres.len() - 1 - centres.len() / 200);
        for axis in 0..3 {
            values.iter_mut().zip(centres).for_each(|(value, c)| *value = c[axis]);
            lo[axis] = *values.select_nth_unstable_by(a, f32::total_cmp).1;
            hi[axis] = *values.select_nth_unstable_by(b, f32::total_cmp).1;
        }
    }
    (centre, lo, hi)
}

fn pool_geometry_candidate(centres: &[[f32; 3]]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let Some(&nan) = centres.first() else { return ([0.0; 3], [0.0; 3], [0.0; 3]) };
    (sog::mean_centre(std::slice::from_ref(&nan)), nan, nan)
}

fn time_pool_geometry(centres: &[[f32; 3]], candidate: bool) -> (f64, f64) {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let start = Instant::now();
    if candidate { black_box(pool_geometry_candidate(black_box(centres))); }
    else { black_box(pool_geometry_reference(black_box(centres))); }
    let ns = start.elapsed().as_secs_f64() * 1e9;
    let allocations = (ALLOCATIONS.load(Ordering::Relaxed) - before) as f64;
    (ns, allocations)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().expect("unpacked SOG chunk directory"));
    let rounds: usize = args.get(1).map_or(4, |s| s.parse().expect("round count"));
    let reverse = args.get(2).is_some_and(|s| s == "reverse");
    let bundle = sog::load_dir(dir).expect("real level chunk");
    let mut real = sog::centres(&bundle);
    assert!(real.len() >= 206_056);
    real[0] = [0.0, -0.0, f32::INFINITY];
    real[1] = [f32::NEG_INFINITY, f32::from_bits(0x7fc0_1234), f32::from_bits(0xffc0_4321)];
    real[2] = [f32::from_bits(0x7fa0_0001), f32::from_bits(0xffa0_0001), f32::from_bits(1)];
    let pool = vec![[f32::NAN; 3]; 3_000_320];
    let mut cases = [
        ("leaf", &real[..1_722], 300usize),
        ("upload", &real[..206_056], 20usize),
        ("pool", pool.as_slice(), 2usize),
    ];
    if reverse { cases.reverse(); }
    for (case, centres, repetitions) in cases {
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        let reference = original(centres);
        let reference_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        let candidate = byte_view(centres);
        let candidate_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
        assert_eq!(candidate.as_ref(), reference.as_slice(), "byte view: {case}");
        assert_eq!(fallback(centres), reference, "little-endian fallback: {case}");
        let hash = fingerprint(candidate.as_ref());
        if rounds == 0 {
            println!("{{\"case\":\"{case}\",\"centres\":{},\"reference_allocations\":{reference_allocations},\"candidate_allocations\":{candidate_allocations},\"fingerprint\":\"{hash:016x}\",\"verified_only\":true}}", centres.len());
        }
        for round in 0..rounds {
            let mut results = [(0.0, 0.0); 2];
            for mode in if reverse { [1usize, 0] } else { [0usize, 1] } {
                results[mode] = time(centres, repetitions, mode == 1);
            }
            println!("{{\"case\":\"{case}\",\"round\":{round},\"centres\":{},\"repetitions\":{repetitions},\"reference_ns\":{},\"candidate_ns\":{},\"reference_allocations\":{},\"candidate_allocations\":{},\"fingerprint\":\"{hash:016x}\"}}", centres.len(), results[0].0, results[1].0, results[0].1, results[1].1);
        }
    }
    for (case, count, repetitions) in [("release-leaf", 1_722usize, 300usize), ("release-upload", 206_056, 20)] {
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        let reference = release_original(count);
        let reference_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        let single = release_single_allocation(count);
        let candidate_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
        assert_eq!(reference, single, "release NaN bytes: {case}");
        let hash = fingerprint(&single);
        if rounds == 0 {
            println!("{{\"case\":\"{case}\",\"centres\":{count},\"reference_allocations\":{reference_allocations},\"candidate_allocations\":{candidate_allocations},\"fingerprint\":\"{hash:016x}\",\"verified_only\":true}}");
        }
        for round in 0..rounds {
            let mut results = [(0.0, 0.0); 2];
            for mode in if reverse { [1usize, 0] } else { [0usize, 1] } {
                results[mode] = time_release(count, repetitions, mode == 1);
            }
            println!("{{\"case\":\"{case}\",\"round\":{round},\"centres\":{count},\"repetitions\":{repetitions},\"reference_ns\":{},\"candidate_ns\":{},\"reference_allocations\":{},\"candidate_allocations\":{},\"fingerprint\":\"{hash:016x}\"}}", results[0].0, results[1].0, results[0].1, results[1].1);
        }
    }
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let reference = pool_geometry_reference(&pool);
    let reference_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let candidate = pool_geometry_candidate(&pool);
    let candidate_allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let bits = |data: ([f32; 3], [f32; 3], [f32; 3])| [data.0, data.1, data.2].map(|row| row.map(f32::to_bits));
    assert_eq!(bits(reference), bits(candidate), "NaN pool geometry");
    let hash = fingerprint(&bits(reference).into_iter().flatten().flat_map(u32::to_le_bytes).collect::<Vec<_>>());
    if rounds == 0 {
        println!("{{\"case\":\"pool-geometry\",\"centres\":{},\"reference_allocations\":{reference_allocations},\"candidate_allocations\":{candidate_allocations},\"fingerprint\":\"{hash:016x}\",\"verified_only\":true}}", pool.len());
    }
    for round in 0..rounds {
        let mut results = [(0.0, 0.0); 2];
        for mode in if reverse { [1usize, 0] } else { [0usize, 1] } {
            results[mode] = time_pool_geometry(&pool, mode == 1);
        }
        println!("{{\"case\":\"pool-geometry\",\"round\":{round},\"centres\":{},\"repetitions\":1,\"reference_ns\":{},\"candidate_ns\":{},\"reference_allocations\":{},\"candidate_allocations\":{},\"fingerprint\":\"{hash:016x}\"}}", pool.len(), results[0].0, results[1].0, results[0].1, results[1].1);
    }
}
