//! CPU-only profiler bookkeeping probe. No graphics device or adapter is created.
//! metrics_probe [samples=100000] [rounds=4]
use splat_render::metrics::Metrics;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

struct Counter;
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counter {
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
static ALLOCATOR: Counter = Counter;

fn update(metrics: &mut Metrics, case: &str, i: usize) {
    let value = (i % 17) as f64 * 0.125;
    if case != "counters" {
        for name in ["tick_total", "physics", "collision_tiles", "encode_submit"] {
            metrics.record(std::hint::black_box(name), value);
        }
    }
    if case != "samples" {
        for name in ["ticks", "render_attempts", "pacing_skips", "uniform_cache_hits"] {
            metrics.count(std::hint::black_box(name), (i % 3) as u64);
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let samples: usize = args.first().map_or(100_000, |s| s.parse().unwrap());
    let rounds: usize = args.get(1).map_or(4, |s| s.parse().unwrap());
    for round in 0..rounds {
        for case in ["samples", "counters", "mixed"] {
            let mut metrics = Metrics::default();
            for i in 0..120 {
                update(&mut metrics, case, i);
            }
            let before = ALLOCATIONS.load(Ordering::Relaxed);
            let start = Instant::now();
            for i in 0..samples {
                update(std::hint::black_box(&mut metrics), case, i);
            }
            let ns = start.elapsed().as_secs_f64() * 1e9 / samples as f64;
            let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
            let serialized = metrics.json().to_string();
            let mut fingerprint = 14_695_981_039_346_656_037u64;
            for byte in serialized.bytes() {
                fingerprint = (fingerprint ^ u64::from(byte)).wrapping_mul(1_099_511_628_211);
            }
            println!("{{\"round\":{round},\"case\":\"{case}\",\"samples\":{samples},\"ns_per_update\":{ns},\"allocations\":{allocations},\"profile_fingerprint\":\"{fingerprint:016x}\"}}");
        }
    }
}
