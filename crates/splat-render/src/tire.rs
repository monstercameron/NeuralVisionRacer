//! Bounded, world-anchored tire decals projected onto Gaussian core depth.
use crate::{
    camera::{self, Mat4},
    scene::View,
    COLOR_FORMAT,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
};

pub const CAPACITY: usize = 8192;
pub const LIFETIME: f32 = 120.0;
const SLAB: f32 = 0.30;

#[derive(Clone, Copy, Debug)]
pub struct TireMark {
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub normal: [f32; 3],
    pub forward: [f32; 3],
    pub width: f32,
    pub phase_start: f32,
    pub phase_end: f32,
    pub strength: f32,
    pub smear: f32,
    pub born: f32,
    pub last_deposit: f32,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Prepared {
    pub count: u32,
    pub rect: Option<[u32; 4]>,
}

pub(crate) struct TireLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    instances: wgpu::Buffer,
    indices: wgpu::Buffer,
    group: wgpu::BindGroup,
    marks: Vec<TireMark>,
    revision: Option<u64>,
    time: f32,
    strength: f32,
    prepared: Cell<Prepared>,
    cache: RefCell<Option<(Mat4, u32, u32, u64)>>,
    uploaded: Cell<u64>,
    frame_upload: Cell<u64>,
    metrics: RefCell<crate::metrics::Metrics>,
}

impl TireLayer {
    pub fn new(device: &wgpu::Device, level: &wgpu::TextureView, car: &wgpu::TextureView) -> Self {
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let uniform = buffer(
            "tire view",
            80,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let instances = buffer(
            "tire segments",
            (CAPACITY * 96) as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        use wgpu::util::DeviceExt;
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tire quad"),
            contents: &[0, 0, 1, 0, 2, 0, 2, 0, 1, 0, 3, 0],
            usage: wgpu::BufferUsages::INDEX,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tire marks"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                depth_entry(2),
                depth_entry(3),
            ],
        });
        let group = bind(device, &layout, &uniform, &instances, level, car);
        let module = crate::shader_module(
            device,
            "tire marks",
            include_str!("shaders/tire_marks.wgsl"),
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tire marks"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tire marks"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    // Black premultiplied alpha darkens existing linear road/shadow color. Preserve target alpha.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            uniform,
            instances,
            indices,
            group,
            marks: Vec::new(),
            revision: None,
            time: 0.0,
            strength: 0.0,
            prepared: Cell::new(Prepared::default()),
            cache: RefCell::new(None),
            uploaded: Cell::new(0),
            frame_upload: Cell::new(0),
            metrics: RefCell::new(Default::default()),
        }
    }
    pub fn rebind(
        &mut self,
        device: &wgpu::Device,
        level: &wgpu::TextureView,
        car: &wgpu::TextureView,
    ) {
        self.group = bind(
            device,
            &self.layout,
            &self.uniform,
            &self.instances,
            level,
            car,
        );
        *self.cache.get_mut() = None;
    }
    pub fn set(&mut self, revision: u64, marks: &VecDeque<TireMark>, time: f32, strength: f32) {
        self.time = time;
        self.strength = if strength.is_finite() {
            strength.clamp(0.0, 2.0)
        } else {
            0.0
        };
        if self.revision != Some(revision) {
            self.marks.clear();
            self.marks
                .extend(marks.iter().rev().take(CAPACITY).rev().copied());
            self.revision = Some(revision);
            *self.cache.get_mut() = None;
        }
    }
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        view: &View,
        clock: Option<fn() -> f64>,
    ) -> Prepared {
        self.frame_upload.set(0);
        if self.strength <= 0.0 || self.marks.is_empty() {
            self.prepared.set(Prepared::default());
            *self.cache.borrow_mut() = None;
            return Prepared::default();
        }
        let start = clock.map(|c| c());
        let vp = camera::mul(&view.projection, &view.view);
        let Some(inverse) = camera::inverse(&vp) else {
            return Prepared::default();
        };
        let mut uniform = inverse.to_vec();
        uniform.extend([
            self.time,
            self.strength,
            view.width as f32,
            view.height as f32,
        ]);
        queue.write_buffer(&self.uniform, 0, &bytes(&uniform));
        let key = (vp, view.width, view.height, self.revision.unwrap_or(0));
        if self.cache.borrow().as_ref() == Some(&key) {
            return self.prepared.get();
        }
        let eye = camera::inverse(&view.view).map_or([0.0; 3], |m| [m[12], m[13], m[14]]);
        let mut data = Vec::with_capacity(self.marks.len() * 24);
        let mut union = None;
        for m in &self.marks {
            let center = std::array::from_fn(|i| (m.start[i] + m.end[i]) * 0.5);
            if dot(sub(center, eye), sub(center, eye)) > 180.0 * 180.0 {
                continue;
            }
            let Some(rect) = projected_rect(m, &vp, view.width, view.height) else {
                continue;
            };
            union = union_rect(union, Some(rect));
            let w = view.width as f32;
            let h = view.height as f32;
            let ndc = [
                rect[0] as f32 / w * 2.0 - 1.0,
                1.0 - (rect[1] + rect[3]) as f32 / h * 2.0,
                (rect[0] + rect[2]) as f32 / w * 2.0 - 1.0,
                1.0 - rect[1] as f32 / h * 2.0,
            ];
            data.extend([
                m.start[0],
                m.start[1],
                m.start[2],
                m.strength,
                m.end[0],
                m.end[1],
                m.end[2],
                m.smear,
                m.normal[0],
                m.normal[1],
                m.normal[2],
                m.last_deposit,
                m.forward[0],
                m.forward[1],
                m.forward[2],
                m.width,
                m.phase_start,
                m.phase_end,
                SLAB,
                0.0,
                ndc[0],
                ndc[1],
                ndc[2],
                ndc[3],
            ]);
        }
        let n = (data.len() * 4) as u64;
        if n > 0 {
            queue.write_buffer(&self.instances, 0, &bytes(&data));
        }
        self.uploaded.set(self.uploaded.get() + n);
        self.frame_upload.set(n);
        let prepared = Prepared {
            count: (data.len() / 24) as u32,
            rect: union,
        };
        self.prepared.set(prepared);
        *self.cache.borrow_mut() = Some(key);
        if let (Some(clock), Some(start)) = (clock, start) {
            self.metrics
                .borrow_mut()
                .record("prepare_cull_upload", clock() - start);
        }
        prepared
    }
    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        count: u32,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tire marks"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            timestamp_writes,
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..6, 0, 0..count);
    }
    pub fn profile_json(&self) -> serde_json::Value {
        let describe = |m: &TireMark| serde_json::json!({"start":m.start,"end":m.end,"strength":m.strength,"smear":m.smear,"born":m.born,"last_deposit":m.last_deposit});
        serde_json::json!({"active":self.marks.len(),"visible":self.prepared.get().count,"capacity":CAPACITY,
            "strength":self.strength,"instance_bytes":96,"uploaded_bytes":self.uploaded.get(),"frame_upload_bytes":self.frame_upload.get(),
            "depth_scissor":self.prepared.get().rect,"projection_slab_m":SLAB,"cpu":self.metrics.borrow().json(),
            "oldest":self.marks.first().map(describe),"newest":self.marks.last().map(describe),
            "strongest":self.marks.iter().max_by(|a,b|a.strength.total_cmp(&b.strength)).map(describe)})
    }
    pub fn reset_profile(&self) {
        self.metrics.borrow_mut().reset();
        self.uploaded.set(0);
        self.frame_upload.set(0);
    }
}
fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn depth_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    instances: &wgpu::Buffer,
    level: &wgpu::TextureView,
    car: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tire marks"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: instances.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(level),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(car),
            },
        ],
    })
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
/// UVs intersect the contact plane, so only its footprint needs rasterizing. Depth still validates
/// the full slab. Extruding these bounds would multiply overdraw on low driving-camera angles.
fn projected_rect(mark: &TireMark, vp: &Mat4, width: u32, height: u32) -> Option<[u32; 4]> {
    let raw_delta = sub(mark.end, mark.start);
    let height_delta = dot(raw_delta, mark.normal);
    let delta: [f32; 3] = std::array::from_fn(|i| raw_delta[i] - mark.normal[i] * height_delta);
    let length = dot(delta, delta).sqrt();
    if length < 1e-5 || !mark.width.is_finite() || mark.width <= 0.0 {
        return None;
    }
    let dir = delta.map(|v| v / length);
    let right = cross(mark.normal, dir);
    let center: [f32; 3] = std::array::from_fn(|i| mark.start[i] + delta[i] * 0.5);
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    let mut front = false;
    let mut behind = false;
    let mut outside = [true; 6];
    for a in [-1.0, 1.0] {
        for b in [-1.0, 1.0] {
            let p: [f32; 3] = std::array::from_fn(|i| {
                center[i] + dir[i] * (length * 0.5 + 0.02) * a + right[i] * mark.width * 0.5 * b
            });
            let clip: [f32; 4] = std::array::from_fn(|i| {
                vp[i] * p[0] + vp[4 + i] * p[1] + vp[8 + i] * p[2] + vp[12 + i]
            });
            if !clip.iter().all(|v| v.is_finite()) {
                return None;
            }
            let rejected = [
                clip[0] < -clip[3],
                clip[0] > clip[3],
                clip[1] < -clip[3],
                clip[1] > clip[3],
                clip[2] < 0.0,
                clip[2] > clip[3],
            ];
            for i in 0..6 {
                outside[i] &= rejected[i];
            }
            if clip[3] <= 0.001 {
                behind = true;
                continue;
            }
            front = true;
            let xy = [
                (clip[0] / clip[3] + 1.0) * 0.5 * width as f32,
                (1.0 - clip[1] / clip[3]) * 0.5 * height as f32,
            ];
            for i in 0..2 {
                lo[i] = lo[i].min(xy[i]);
                hi[i] = hi[i].max(xy[i]);
            }
        }
    }
    if !front || outside.into_iter().any(|v| v) {
        return None;
    }
    if behind {
        return Some([0, 0, width, height]);
    }
    let x = lo[0].floor().clamp(0.0, width as f32) as u32;
    let y = lo[1].floor().clamp(0.0, height as f32) as u32;
    let xmax = hi[0].ceil().clamp(0.0, width as f32) as u32;
    let ymax = hi[1].ceil().clamp(0.0, height as f32) as u32;
    (xmax > x && ymax > y).then_some([x, y, xmax - x, ymax - y])
}
pub(crate) fn union_rect(a: Option<[u32; 4]>, b: Option<[u32; 4]>) -> Option<[u32; 4]> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => {
            let x = a[0].min(b[0]);
            let y = a[1].min(b[1]);
            Some([
                x,
                y,
                (a[0] + a[2]).max(b[0] + b[2]) - x,
                (a[1] + a[3]).max(b[1] + b[3]) - y,
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn footprint_culling_keeps_bounds_small_and_rejects_hidden_frustum_regions() {
        let vp = camera::mul(
            &camera::perspective(60f32.to_radians(), 1.0, 0.01, 100.0),
            &camera::look_at([0.0, 3.0, 3.0], [0.0; 3], [0.0, 1.0, 0.0]),
        );
        let mark = TireMark {
            start: [0.0, 0.0, 0.0],
            end: [0.0, 0.0, 0.25],
            normal: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, 1.0],
            width: 0.3,
            phase_start: 0.0,
            phase_end: 0.25,
            strength: 0.5,
            smear: 0.0,
            born: 0.0,
            last_deposit: 0.0,
        };
        let r = projected_rect(&mark, &vp, 128, 128).unwrap();
        assert!(
            r[2] * r[3] < 200,
            "a small road footprint must not rasterize its whole projection slab"
        );
        assert!(projected_rect(
            &TireMark {
                start: [0.0, 0.0, 30.0],
                end: [0.0, 0.0, 30.25],
                ..mark
            },
            &vp,
            128,
            128
        )
        .is_none());
        assert!(projected_rect(
            &TireMark {
                start: [30.0, 0.0, 0.0],
                end: [30.0, 0.0, 0.25],
                ..mark
            },
            &vp,
            128,
            128
        )
        .is_none());
    }
}
