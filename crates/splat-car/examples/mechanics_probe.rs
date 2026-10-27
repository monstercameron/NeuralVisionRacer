//! CPU-only mechanics throughput/allocation probe. No physics engine or GPU is created.
//! mechanics_probe [samples=100000] [rounds=4]
use splat_car::{Car, CarSpec, Controls, Output};
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
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout)
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.realloc(p, layout, size)
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;
fn hash_output(hash: &mut u64, o: Output) {
    for v in [
        o.drive_force_n,
        o.axle_torque_nm,
        o.drive_ratio,
        o.brake_front_n,
        o.brake_rear_n,
        o.handbrake_rear_n,
        o.throttle,
        o.brake,
        o.handbrake,
        o.drag_n,
        o.rolling_n,
        o.downforce_n,
        o.steer_angle,
        o.rpm,
        o.engine_torque_nm,
        o.power_kw,
        o.speed_mps,
        o.traction_excess,
        o.water_temp_c,
        o.oil_temp_c,
        o.oil_pressure_bar,
        o.boost_bar,
    ] {
        *hash = (*hash ^ u64::from(v.to_bits())).wrapping_mul(1_099_511_628_211);
    }
    for v in [
        o.gear as u64,
        o.shifting as u64,
        o.limiter as u64,
        o.wheelspin as u64,
        o.launch as u64,
    ] {
        *hash = (*hash ^ v).wrapping_mul(1_099_511_628_211);
    }
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let n: usize = args.first().map_or(100_000, |s| s.parse().unwrap());
    let rounds: usize = args.get(1).map_or(4, |s| s.parse().unwrap());
    for round in 0..rounds {
        for case in ["cruise", "mixed", "spec_edits"] {
            let mut car = Car::new(CarSpec::porsche_992_gt3_rs());
            for _ in 0..64 {
                std::hint::black_box(car.step(Controls::default(), 0.0, 1.0 / 240.0));
            }
            let mut hash = 14_695_981_039_346_656_037;
            let alloc_before = ALLOCATIONS.load(Ordering::Relaxed);
            let start = Instant::now();
            for i in 0..n {
                if case == "spec_edits" && i % 4096 == 0 {
                    car.spec.torque_curve[2].1 = if i % 8192 == 0 { 430.0 } else { 435.0 };
                    car.spec.idle_rpm = if i % 8192 == 0 { 1000.0 } else { 1001.0 };
                    car.spec.limiter_rpm = if i % 8192 == 0 { 9000.0 } else { 9050.0 };
                }
                let phase = (i / 1024) % 8;
                let (controls, speed) = if case == "cruise" {
                    (
                        Controls {
                            throttle: 0.4,
                            ..Default::default()
                        },
                        20.0,
                    )
                } else {
                    (
                        Controls {
                            throttle: if phase < 4 { 1.0 } else { 0.0 },
                            brake: if phase >= 4 { 0.8 } else { 0.0 },
                            steer: ((i % 101) as f32 - 50.0) * 0.01,
                            shift: if i % 2048 == 0 { 1 } else { 0 },
                            handbrake: if phase == 7 { 1.0 } else { 0.0 },
                        },
                        if phase == 0 || phase == 4 {
                            0.0
                        } else if phase > 4 {
                            -((i % 1200) as f32) * 0.01
                        } else {
                            (i % 7000) as f32 * 0.01
                        },
                    )
                };
                let out = std::hint::black_box(car.step(
                    std::hint::black_box(controls),
                    std::hint::black_box(speed),
                    if i % 2 == 0 { 1.0 / 120.0 } else { 1.0 / 240.0 },
                ));
                hash_output(&mut hash, out);
            }
            let elapsed = start.elapsed().as_secs_f64() * 1e9;
            let allocations = ALLOCATIONS.load(Ordering::Relaxed) - alloc_before;
            println!("{{\"round\":{round},\"case\":\"{case}\",\"samples\":{n},\"ns_per_step\":{},\"allocations\":{allocations},\"output_fingerprint\":\"{hash:016x}\"}}",elapsed/n as f64);
        }
    }
}
