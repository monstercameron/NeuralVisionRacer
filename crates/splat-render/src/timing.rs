//! Actual pass-boundary timestamps, sampled without blocking. A sort stays batched for performance.
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
const MAX_PASSES: usize = 256;
const QUERY_COUNT: u32 = MAX_PASSES as u32 * 2;
const BYTES: u64 = QUERY_COUNT as u64 * 8;
#[derive(Clone)]
struct Event {
    id: &'static str,
    target: &'static str,
    draws: u64,
    dispatches: u64,
    workgroups: u64,
    items: u64,
}
#[derive(Clone)]
struct Sample {
    frame: u64,
    passes: Vec<(Event, f64, Option<f64>)>,
    total_ms: f64,
}
#[derive(Default)]
struct History {
    epoch: u64,
    samples: VecDeque<Sample>,
    map_errors: u64,
    overflow: u64,
    busy_skips: u64,
}
struct Slot {
    buffer: wgpu::Buffer,
    busy: Arc<AtomicBool>,
    events: Mutex<Vec<Event>>,
    frame: Cell<u64>,
}
pub(crate) struct GpuTimer {
    queries: wgpu::QuerySet,
    frames: Cell<u64>,
    interval: Cell<u64>,
    active: Cell<bool>,
    events: RefCell<Vec<Event>>,
    resolve: wgpu::Buffer,
    slots: [Slot; 3],
    period_ns: f64,
    result: Arc<AtomicU64>,
    history: Arc<Mutex<History>>,
}
impl GpuTimer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, result: Arc<AtomicU64>) -> Self {
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("profile pass timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: QUERY_COUNT,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("profile resolve"),
            size: BYTES,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slots = std::array::from_fn(|_| Slot {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("profile readback"),
                size: BYTES,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            busy: Arc::new(AtomicBool::new(false)),
            events: Default::default(),
            frame: Cell::new(0),
        });
        Self {
            queries,
            frames: Cell::new(0),
            interval: Cell::new(8),
            active: Cell::new(false),
            events: Default::default(),
            resolve,
            slots,
            period_ns: f64::from(queue.get_timestamp_period()),
            result,
            history: Default::default(),
        }
    }
    pub fn begin(&self) -> Option<usize> {
        self.active.set(false);
        self.events.borrow_mut().clear();
        let f = self.frames.get();
        self.frames.set(f.wrapping_add(1));
        if f % self.interval.get() != 0 {
            return None;
        }
        let Some(i) = self.slots.iter().position(|s| {
            s.busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        }) else {
            self.history.lock().unwrap().busy_skips += 1;
            return None;
        };
        self.slots[i].frame.set(f);
        self.active.set(true);
        Some(i)
    }
    fn indices(
        &self,
        id: &'static str,
        target: &'static str,
        draws: u64,
        dispatches: u64,
        workgroups: u64,
        items: u64,
    ) -> Option<u32> {
        if !self.active.get() {
            return None;
        }
        let mut events = self.events.borrow_mut();
        if events.len() == MAX_PASSES {
            self.history.lock().unwrap().overflow += 1;
            return None;
        }
        let index = events.len() as u32 * 2;
        events.push(Event {
            id,
            target,
            draws,
            dispatches,
            workgroups,
            items,
        });
        Some(index)
    }
    pub fn render(
        &self,
        id: &'static str,
        target: &'static str,
        draws: u64,
        items: u64,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let i = self.indices(id, target, draws, 0, 0, items)?;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(i),
            end_of_pass_write_index: Some(i + 1),
        })
    }
    pub fn compute(
        &self,
        id: &'static str,
        target: &'static str,
        dispatches: u64,
        workgroups: u64,
        items: u64,
    ) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        let i = self.indices(id, target, 0, dispatches, workgroups, items)?;
        Some(wgpu::ComputePassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(i),
            end_of_pass_write_index: Some(i + 1),
        })
    }
    pub fn end(&self, encoder: &mut wgpu::CommandEncoder, slot: usize) {
        self.active.set(false);
        let n = self.events.borrow().len() as u32 * 2;
        if n == 0 {
            self.slots[slot].busy.store(false, Ordering::Release);
            return;
        }
        *self.slots[slot].events.lock().unwrap() = self.events.borrow().clone();
        encoder.resolve_query_set(&self.queries, 0..n, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            0,
            &self.slots[slot].buffer,
            0,
            u64::from(n) * 8,
        );
    }
    pub fn read(&self, slot: usize) {
        let events = std::mem::take(&mut *self.slots[slot].events.lock().unwrap());
        if events.is_empty() {
            return;
        }
        let buffer = self.slots[slot].buffer.clone();
        let mapped = buffer.clone();
        let busy = self.slots[slot].busy.clone();
        let result = self.result.clone();
        let period = self.period_ns;
        let history = self.history.clone();
        let epoch = history.lock().unwrap().epoch;
        let frame = self.slots[slot].frame.get();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |ok| {
            let mut sample = None;
            if ok.is_ok() {
                if let Ok(bytes) = mapped.slice(..).get_mapped_range() {
                    let raw: Vec<u64> = bytes[..events.len() * 16]
                        .chunks_exact(8)
                        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                        .collect();
                    let first = raw
                        .iter()
                        .step_by(2)
                        .copied()
                        .filter(|n| *n > 0)
                        .min()
                        .unwrap_or(0);
                    let last = raw
                        .chunks_exact(2)
                        .filter(|r| r[1] >= r[0])
                        .map(|r| r[1])
                        .max();
                    // Metal can leave the end timestamp unwritten for a clear-only pass. Keep that event but mark its
                    // duration unavailable; never invent a zero cost or discard the real graphics measurements.
                    if let Some(last) = last.filter(|last| *last >= first) {
                        let total_ms = (last - first) as f64 * period / 1e6;
                        sample = Some(Sample {
                            frame,
                            total_ms,
                            passes: events
                                .into_iter()
                                .zip(raw.chunks_exact(2))
                                .map(|(e, r)| {
                                    let duration =
                                        (r[1] >= r[0]).then(|| (r[1] - r[0]) as f64 * period / 1e6);
                                    (
                                        e,
                                        r[0].saturating_sub(first) as f64 * period / 1e6,
                                        duration,
                                    )
                                })
                                .collect(),
                        });
                    }
                }
                mapped.unmap();
            }
            let mut h = history.lock().unwrap();
            if h.epoch == epoch {
                if let Some(s) = sample {
                    if h.samples.iter().all(|p| s.frame > p.frame) {
                        result.store(s.total_ms.to_bits(), Ordering::Relaxed);
                    }
                    if h.samples.len() == 120 {
                        h.samples.pop_front();
                    }
                    h.samples.push_back(s);
                } else {
                    h.map_errors += 1;
                }
            }
            busy.store(false, Ordering::Release);
        });
    }
    pub fn interval(&self) -> u64 {
        self.interval.get()
    }
    pub fn set_interval(&self, n: u64) {
        self.interval.set(n.clamp(1, 600));
    }
    pub fn reset(&self) {
        let mut h = self.history.lock().unwrap();
        let epoch = h.epoch + 1;
        *h = History {
            epoch,
            ..Default::default()
        };
    }
    pub fn snapshot(&self) -> Value {
        let h = self.history.lock().unwrap();
        let latest = h.samples.iter().max_by_key(|s| s.frame);
        let mut grouped: BTreeMap<(&str, &str), Vec<f64>> = BTreeMap::new();
        for s in &h.samples {
            let mut per_frame: BTreeMap<(&str, &str), f64> = BTreeMap::new();
            for (e, _, ms) in &s.passes {
                if let Some(ms) = ms {
                    *per_frame.entry((e.id, e.target)).or_default() += ms;
                }
            }
            for (k, v) in per_frame {
                grouped.entry(k).or_default().push(v);
            }
        }
        json!({"sample_every_frames":self.interval(),"sampled_frames":h.samples.len(),"latest_sample_frame":latest.map(|s|s.frame),"gpu_frame_ms":crate::metrics::summary(h.samples.iter().map(|s|s.total_ms)),"pass_sum_note":"Measured passes only; unavailable timestamps are null. GPU pass execution may overlap, so this sum is not frame time.","gpu_pass_sum_ms":latest.map(|s|s.passes.iter().filter_map(|p|p.2).sum::<f64>()),"pass_aggregates":grouped.iter().map(|((id,target),v)|json!({"id":id,"target":target,"gpu_ms":crate::metrics::summary(v.iter().copied())})).collect::<Vec<_>>(),"passes":latest.map(|s|s.passes.iter().map(|(e,start,ms)|json!({"id":e.id,"target":e.target,"start_ms":start,"gpu_ms":ms,"timing_status":if ms.is_some(){"measured"}else{"timestamp_unavailable"},"draw_calls":e.draws,"dispatches":e.dispatches,"workgroups":e.workgroups,"input_items":e.items})).collect::<Vec<_>>()).unwrap_or_default(),"readback_errors":h.map_errors,"query_overflow":h.overflow,"readback_busy_skips":h.busy_skips})
    }
}
/// Registry includes conditional passes even when absent from the latest sampled frame.
pub(crate) fn catalogue() -> Value {
    json!([
    {"id":"splat_sort","kind":"compute","shaders":["splat-render/src/shaders/depth_keys.wgsl","splat-render/src/shaders/draw_args.wgsl","splat-wgsl/shaders/radix-sort/compute-radix-sort-4bit.wgsl","splat-wgsl/shaders/radix-sort/compute-prefix-sum.wgsl","splat-wgsl/shaders/radix-sort/compute-radix-sort-reorder.wgsl"],"effects":["depth keys","frustum/cutout culling","visible count","radix histogram/scan/reorder"],"timing_scope":"whole batched compute pass; individual dispatches share the measurement"},
    {"id":"wheel_deformation","kind":"compute","shaders":["splat-render/src/shaders/wheel_centers.wgsl"],"effects":["wheel centre rotation/steering/suspension"]},
    {"id":"sky","kind":"render","shaders":["splat-render/src/shaders/sky.wgsl"]},
    {"id":"level_depth","kind":"render","shaders":["splat-wgsl/shaders/gsplat/vert/gsplat.wgsl [PREPASS_PASS]","splat-wgsl/shaders/gsplat/frag/gsplat.wgsl [PREPASS_PASS]"]},
    {"id":"car_depth","kind":"render","shaders":["splat-wgsl/shaders/gsplat/vert/gsplat.wgsl [PREPASS_PASS]","splat-wgsl/shaders/gsplat/frag/gsplat.wgsl [PREPASS_PASS]"]},
    {"id":"dust","kind":"render","shaders":["splat-render/src/shaders/dust.wgsl"],"effects":["ground material / speed / slip emission","sorted soft particles","car and road depth fade","simulation-time motion"]},
    {"id":"tire_marks","kind":"render","shaders":["splat-render/src/shaders/tire_marks.wgsl"],"effects":["contact tread / slip smear","Gaussian-depth projection","car occlusion","simulation-time fading"]},
    {"id":"level_color","kind":"render","shaders":["splat-wgsl/shaders/gsplat/vert/gsplat.wgsl","splat-wgsl/shaders/gsplat/frag/gsplat.wgsl","splat-wgsl/src/local/carShadowVS.wgsl"],"effects":["SH colour","surface-fitted shadow / receiver incidence","LOD crossfade"]},
    {"id":"car_color","kind":"render","shaders":["splat-wgsl/shaders/gsplat/vert/gsplat.wgsl","splat-wgsl/shaders/gsplat/frag/gsplat.wgsl","splat-wgsl/src/local/carShadowVS.wgsl","splat-wgsl/src/local/carClarityPS.wgsl","splat-wgsl/src/local/wheelMath.wgsl"],"effects":["wheel Gaussian orientation/SH","sun clearcoat with shared proxy occlusion","haze alpha cutoff"]},
    {"id":"depth_of_field","kind":"render","shaders":["splat-render/src/shaders/dof.wgsl"]},
    {"id":"post","kind":"render","shaders":["splat-render/src/shaders/post.wgsl"],"effects":["car sharpen/clarity","blur","fringe","grain","grade"],"timing_scope":"combined shader; use controlled effect toggles to isolate contributions"},
    {"id":"debug_lines","kind":"render","shaders":["splat-render/src/shaders/debug_lines.wgsl"]},
    {"id":"hud","kind":"render","shaders":["splat-render/src/shaders/overlay.wgsl"]},
    {"id":"present_blit","kind":"render","shaders":["splat-render/src/shaders/blit.wgsl"],"timing_scope":"GPU blit only; excludes compositor/vsync"}
    ])
}
