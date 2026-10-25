//! Uploads a decoded SOG bundle and draws it through the composed gsplat pipelines.
//!
//! Each frame the splats are sorted back-to-front by view depth on the GPU (`sort::DepthSorter`, a port of the
//! engine's radix sort). Each instance covers `INSTANCE_SIZE` splats, as the engine's
//! `GSplatResourceBase.instanceSize`, so the quad buffer holds that many six-vertex quads.
//!
//! A scene draws two ways: in colour (sorted, blended, depth tested but not written), and depth only (the engine's
//! `PREPASS_PASS` variant: fragments below `alphaClip` are discarded and the rest write depth). The frame uses the
//! depth pass so separately sorted objects, the level and the car, hide each other (see `frame`).

use crate::camera::{self, Mat4};
use crate::env::Sky;
use crate::sog::{self, Bundle};
use crate::sort::DepthSorter;
use crate::{bind_group_layouts, gsplat_depth_pipeline, gsplat_pipeline, shader_module, DEPTH_FORMAT};
use splat_wgsl::binding::{Binding, BindingKind};
use splat_wgsl::pipeline::{compose_pipeline, compose_pipeline_with};
use std::collections::BTreeMap;
use std::sync::mpsc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Splats per instance (`GSplatResourceBase.instanceSize` in the engine).
pub const INSTANCE_SIZE: u32 = 128;

/// The engine's 32×32 RGBA8 blue-noise texture (`core/math/blue-noise.js`), row-major.
const BLUE_NOISE_32: &[u8] = include_bytes!("../assets/blue_noise_32_rgba8.bin");

/// Camera and viewport for one frame. `model` is the moving object's (the car's) splat-to-world matrix.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub model: Mat4,
    pub view: Mat4,
    pub projection: Mat4,
    pub width: u32,
    pub height: u32,
    pub near: f32,
    pub far: f32,
}

/// One composed pipeline with the binding table its bind groups follow.
struct Variant {
    pipeline: wgpu::RenderPipeline,
    layouts: Vec<wgpu::BindGroupLayout>,
    bindings: Vec<Binding>,
}

/// The colour and depth-prepass pipelines, compiled once and shared by every scene.
pub struct Pipelines {
    color: Variant,
    depth: Variant,
}

impl Pipelines {
    /// Composes and compiles both variants for `color_format`.
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Result<Self> {
        let (vs, fs, bindings) = compose_pipeline().map_err(|e| format!("compose: {e:?}"))?;
        let (vs, fs) = (shader_module(device, "gsplat vs", &vs), shader_module(device, "gsplat fs", &fs));
        let layouts = bind_group_layouts(device, &bindings);
        let color = Variant { pipeline: gsplat_pipeline(device, &vs, &fs, &layouts, color_format), layouts, bindings };

        // Depth needs no colour, so the prepass skips the spherical-harmonics evaluation in the vertex stage.
        let (vs, fs, bindings) = compose_pipeline_with(&["PREPASS_PASS", "SH_BANDS=0"]).map_err(|e| format!("compose prepass: {e:?}"))?;
        let (vs, fs) = (shader_module(device, "gsplat depth vs", &vs), shader_module(device, "gsplat depth fs", &fs));
        let layouts = bind_group_layouts(device, &bindings);
        let depth = Variant { pipeline: gsplat_depth_pipeline(device, &vs, &fs, &layouts), layouts, bindings };
        Ok(Self { color, depth })
    }
}

/// A splat asset resident on the GPU, with its bind groups and sorter.
pub struct GsplatScene {
    color_groups: Vec<wgpu::BindGroup>,
    depth_groups: Vec<wgpu::BindGroup>,
    /// Uniform and storage buffers, keyed by the binding name from the tables.
    buffers: BTreeMap<String, wgpu::Buffer>,
    quad: wgpu::Buffer,
    count: u32,
    sorter: DepthSorter,
    /// Mean splat centre in splat space, for ordering several scenes back to front.
    pub centre: [f32; 3],
    /// Splat-space bounds of the splats between the 0.5th and 99.5th percentiles per axis, for screen-space culling.
    pub bounds: ([f32; 3], [f32; 3]),
    /// The splat textures by stream name, kept so a pool can be written in place.
    textures: BTreeMap<String, wgpu::Texture>,
    centers: wgpu::Buffer,
    splat_width: u32,
}

/// Uploads the bundle's textures, codebook and centres for `pipelines`. In the depth pass, fragments with alpha
/// below `depth_alpha_clip` are discarded, so it sets how solid a splat must be to hide what is behind it.
pub fn build(device: &wgpu::Device, queue: &wgpu::Queue, pipelines: &Pipelines, bundle: &Bundle, depth_alpha_clip: f32) -> Result<GsplatScene> {
    let textures = bundle
        .textures
        .iter()
        .map(|(name, tex)| (name.to_string(), upload_texture(device, queue, name, tex.width, tex.height, wgpu::TextureFormat::Rgba8Unorm, Some(&tex.rgba))))
        .collect();
    let width = bundle.textures["means_l"].width;
    assemble(device, queue, pipelines, &bundle.meta, textures, &sog::centres(bundle), width, depth_alpha_clip)
}

/// Width of a streaming pool's per-splat textures.
pub const POOL_WIDTH: u32 = 2048;
/// Most rows of a pool's SH centroid texture (64 centroids per row); the device's largest texture side may cap it.
pub const POOL_CENTROID_ROWS: u32 = 16384;

/// An empty splat pool of `capacity` slots for streaming (`stream::LevelStream` fills it). `meta` gives the shared
/// position range and codebooks every slot is encoded with; its count is ignored. Every slot starts empty: its
/// centre is NaN, which the sort culls.
pub fn pool(device: &wgpu::Device, queue: &wgpu::Queue, pipelines: &Pipelines, meta: &sog::Meta, capacity: u32, depth_alpha_clip: f32) -> Result<GsplatScene> {
    let rows = capacity.div_ceil(POOL_WIDTH);
    let mut textures: BTreeMap<String, wgpu::Texture> = ["means_l", "means_u", "quats", "scales", "sh0", "sh_labels"]
        .iter()
        .map(|name| (name.to_string(), upload_texture(device, queue, name, POOL_WIDTH, rows, wgpu::TextureFormat::Rgba8Unorm, None)))
        .collect();
    textures.insert(
        "sh_centroids".into(),
        upload_texture(device, queue, "sh_centroids", 64 * 15, POOL_CENTROID_ROWS.min(device.limits().max_texture_dimension_2d), wgpu::TextureFormat::Rgba8Unorm, None),
    );
    let meta = sog::Meta { count: rows * POOL_WIDTH, ..meta.clone() };
    let empty = vec![[f32::NAN; 3]; meta.count as usize];
    assemble(device, queue, pipelines, &meta, textures, &empty, POOL_WIDTH, depth_alpha_clip)
}

/// The shared part of `build` and `pool`: codebook, samplers, buffers, sorter and bind groups around `textures`.
#[allow(clippy::too_many_arguments)]
fn assemble(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &Pipelines,
    meta: &sog::Meta,
    textures: BTreeMap<String, wgpu::Texture>,
    centres: &[[f32; 3]],
    splat_width: u32,
    depth_alpha_clip: f32,
) -> Result<GsplatScene> {
    let count = meta.count;
    let mut views: BTreeMap<String, wgpu::TextureView> = textures.iter().map(|(name, t)| (name.clone(), t.create_view(&Default::default()))).collect();
    let codebook: Vec<u8> = sog::codebook_rgba32f(meta).iter().flat_map(|f| f.to_le_bytes()).collect();
    views.insert(
        "sogCodebook".into(),
        upload_texture(device, queue, "sogCodebook", 256, 1, wgpu::TextureFormat::Rgba32Float, Some(&codebook)).create_view(&Default::default()),
    );
    views.insert(
        "blueNoiseTex32".into(),
        upload_texture(device, queue, "blueNoiseTex32", 32, 32, wgpu::TextureFormat::Rgba8Unorm, Some(BLUE_NOISE_32)).create_view(&Default::default()),
    );
    let nearest = wgpu::SamplerDescriptor {
        label: Some("gsplat nearest"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    };
    let sampler = device.create_sampler(&nearest);
    // The blue-noise lookup wraps, as the engine's `getBlueNoiseTexture` sampler does.
    let blue_noise_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("blue noise repeat"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        ..nearest
    });

    // One buffer per uniform name across both tables, so the passes share the matrices.
    let mut buffers: BTreeMap<String, wgpu::Buffer> = BTreeMap::new();
    for b in pipelines.color.bindings.iter().chain(&pipelines.depth.bindings) {
        if let BindingKind::Uniform { ty } = &b.kind {
            if buffers.contains_key(&b.name) {
                continue;
            }
            let size = if ty == "mat4x4f" { 64 } else { 16 };
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&b.name),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            buffers.insert(b.name.clone(), buffer);
        }
    }
    let order = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splatOrder"),
        size: u64::from(count.max(1)) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    buffers.insert("splatOrder".into(), order);

    let centers = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splat centres"),
        size: u64::from(count.max(1)) * 12,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&centers, 0, &f32s(&centres.concat()));
    // Per-slot opacity for crossfades (see `carShadowVS`): whole until a sort writes otherwise.
    let fade = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splatFade"),
        size: u64::from(count.max(1)) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&fade, 0, &f32s(&vec![1.0; count.max(1) as usize]));
    let sorter = DepthSorter::new(device, queue, count, &centers, &fade);
    buffers.insert("splatFade".into(), fade);
    // Per-slot level of detail (plus one; 0 for splats not from a streamed level) and the LOD tint switch.
    let lod = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splatLod"),
        size: u64::from(count.max(1)) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    buffers.insert("splatLod".into(), lod);
    let debug = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("splatDebug"),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    buffers.insert("splatDebug".into(), debug);

    let scene_buffers = Buffers(&buffers);
    scene_buffers.set(queue, "means_mins", &f32s(&meta.means_mins));
    scene_buffers.set(queue, "means_maxs", &f32s(&meta.means_maxs));
    scene_buffers.set(queue, "splatTextureSize", &u32s(&[splat_width]));
    scene_buffers.set(queue, "numSplats", &u32s(&[count]));
    scene_buffers.set(queue, "minPixelSize", &f32s(&[2.0]));
    scene_buffers.set(queue, "alphaClipForward", &f32s(&[1.0 / 255.0]));
    scene_buffers.set(queue, "alphaClip", &f32s(&[depth_alpha_clip]));
    scene_buffers.set(queue, "blueNoiseJitter", &f32s(&[0.0, 0.0, 0.0, 0.0]));

    let groups = |variant: &Variant| -> Vec<wgpu::BindGroup> {
        variant
            .layouts
            .iter()
            .enumerate()
            .map(|(group, layout)| {
                let entries: Vec<wgpu::BindGroupEntry> = variant
                    .bindings
                    .iter()
                    .filter(|b| b.group == group as u32)
                    .map(|b| {
                        let resource = match &b.kind {
                            BindingKind::Uniform { .. } | BindingKind::Storage { .. } => buffers[&b.name].as_entire_binding(),
                            BindingKind::Texture { .. } => {
                                wgpu::BindingResource::TextureView(views.get(&b.name).expect("texture for binding"))
                            }
                            BindingKind::Sampler { .. } if b.name.starts_with("blueNoise") => {
                                wgpu::BindingResource::Sampler(&blue_noise_sampler)
                            }
                            BindingKind::Sampler { .. } => wgpu::BindingResource::Sampler(&sampler),
                        };
                        wgpu::BindGroupEntry { binding: b.binding, resource }
                    })
                    .collect();
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("gsplat group {group}")),
                    layout,
                    entries: &entries,
                })
            })
            .collect()
    };
    let color_groups = groups(&pipelines.color);
    let depth_groups = groups(&pipelines.depth);

    let quad_data = quad_vertices();
    let quad = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gsplat quads"),
        size: quad_data.len() as u64 * 4,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&quad, 0, &f32s(&quad_data));

    Ok(GsplatScene {
        color_groups,
        depth_groups,
        buffers,
        quad,
        count,
        sorter,
        centre: sog::mean_centre(centres),
        bounds: robust_bounds(centres),
        textures,
        centers,
        splat_width,
    })
}

impl GsplatScene {
    /// Writes `centres.len()` splats into slots from `start`: one RGBA8 texel per splat for each named stream
    /// (`means_l`, `means_u`, `quats`, `scales`, `sh0`, `sh_labels`), tightly packed, and their centres.
    pub fn write_splats(&self, queue: &wgpu::Queue, start: u32, streams: &[(&str, &[u8])], centres: &[[f32; 3]]) {
        let count = centres.len() as u32;
        for (name, texels) in streams {
            if let Some(texture) = self.textures.get(*name) {
                write_slots(queue, texture, self.splat_width, start, count, texels);
            }
        }
        queue.write_buffer(&self.centers, u64::from(start) * 12, &f32s(&centres.concat()));
    }

    /// Empties `count` slots from `start`: their centres become NaN, which the sort culls.
    pub fn release(&self, queue: &wgpu::Queue, start: u32, count: u32) {
        queue.write_buffer(&self.centers, u64::from(start) * 12, &f32s(&vec![f32::NAN; count as usize * 3]));
    }

    /// Records the level of detail and leaf held by `count` slots from `start` (for the debug colours).
    pub fn write_lod(&self, queue: &wgpu::Queue, start: u32, count: u32, lod: usize, leaf: usize) {
        let value = ((leaf as u32) << 4 | (lod as u32 + 1).min(15)).to_le_bytes();
        queue.write_buffer(&self.buffers["splatLod"], u64::from(start) * 4, &value.repeat(count as usize));
    }

    /// Debug colouring of streamed splats (see `carShadowVS`): 0 normal, 1 by level of detail, 2 by distance.
    pub fn set_debug_colour(&self, queue: &wgpu::Queue, mode: u32) {
        queue.write_buffer(&self.buffers["splatDebug"], 0, &mode.to_le_bytes());
    }

    /// Rows of the SH centroid texture.
    pub fn centroid_rows(&self) -> u32 {
        self.textures.get("sh_centroids").map_or(0, |t| t.height())
    }

    /// Writes whole rows of the SH centroid texture (64 centroids × 15 texels per row) from row `first_row`.
    pub fn write_centroid_rows(&self, queue: &wgpu::Queue, first_row: u32, rgba: &[u8]) {
        let texture = &self.textures["sh_centroids"];
        let width = texture.width();
        let rows = (rgba.len() / (width as usize * 4)) as u32;
        if rows == 0 {
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: first_row, z: 0 }, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(rows) },
            wgpu::Extent3d { width, height: rows, depth_or_array_layers: 1 },
        );
    }

    /// Sorts and draws only the first `active` slots.
    pub fn set_active(&self, queue: &wgpu::Queue, active: u32) {
        self.sorter.set_active(queue, active);
    }

    /// Slot ranges crossfading, sorted by start: (first, one past the last, share of the range drawn). See
    /// `DepthSorter::set_fades`.
    pub fn set_fades(&self, queue: &wgpu::Queue, fades: &[(u32, u32, f32)]) {
        self.sorter.set_fades(queue, fades);
    }

    /// Writes a named uniform of the composed shaders (e.g. the car shadow's `shadow_box`). Unknown names are ignored.
    pub fn set_uniform(&self, queue: &wgpu::Queue, name: &str, bytes: &[u8]) {
        Buffers(&self.buffers).set(queue, name, bytes);
    }

    /// Slots in this scene.
    pub fn capacity(&self) -> u32 {
        self.count
    }

    /// Writes this frame's matrices and records the depth sort. Call before any pass that draws the scene.
    pub fn prepare(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, view: &View, model: &Mat4) {
        self.prepare_with_cutout(queue, encoder, view, model, None);
    }

    /// `prepare`, also cutting the splats that hide `cutout`'s target (see `sort::Cutout`).
    pub fn prepare_with_cutout(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, view: &View, model: &Mat4, cutout: Option<&crate::sort::Cutout>) {
        let buffers = Buffers(&self.buffers);
        buffers.set(queue, "matrix_model", &f32s(model));
        buffers.set(queue, "matrix_view", &f32s(&view.view));
        buffers.set(queue, "matrix_projection", &f32s(&view.projection));
        buffers.set(queue, "camera_params", &f32s(&[1.0 / view.far, view.far, view.near, 0.0]));
        let (w, h) = (view.width as f32, view.height as f32);
        buffers.set(queue, "viewport_size", &f32s(&[w, h, 1.0 / w, 1.0 / h]));
        let view_model = camera::mul(&view.view, model);
        let clip = camera::mul(&view.projection, &view_model);
        self.sorter.encode(queue, encoder, &view_model, Some(&clip), cutout.map(|c| (c, &view.view, model)), &self.buffers["splatOrder"]);
    }

    /// Draws the sorted splats, blended, into a pass with a colour target and a depth attachment.
    pub fn draw_color(&self, pass: &mut wgpu::RenderPass<'_>, pipelines: &Pipelines) {
        self.draw(pass, &pipelines.color.pipeline, &self.color_groups);
    }

    /// Writes the splats' depth into a depth-only pass.
    pub fn draw_depth(&self, pass: &mut wgpu::RenderPass<'_>, pipelines: &Pipelines) {
        self.draw(pass, &pipelines.depth.pipeline, &self.depth_groups);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline, groups: &[wgpu::BindGroup]) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(pipeline);
        for (index, group) in groups.iter().enumerate() {
            pass.set_bind_group(index as u32, group, &[]);
        }
        pass.set_vertex_buffer(0, self.quad.slice(..));
        // Only the splats in view: the sort put them first and counted them.
        pass.draw_indirect(self.sorter.draw_args(), 0);
    }

    /// Renders this scene alone offscreen (sky, then splats with `view.model`) and reads it back as tightly packed
    /// RGBA8 rows, top row first. The target is `COLOR_FORMAT` (sRGB), so the bytes are sRGB-encoded.
    pub fn render_rgba(&self, device: &wgpu::Device, queue: &wgpu::Queue, pipelines: &Pipelines, view: &View, sky: Option<(&Sky, f32)>) -> Vec<u8> {
        let extent = wgpu::Extent3d { width: view.width, height: view.height, depth_or_array_layers: 1 };
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gsplat color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gsplat depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());

        let bytes_per_row = (view.width * 4).next_multiple_of(256);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gsplat readback"),
            size: u64::from(bytes_per_row) * u64::from(view.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        self.prepare(queue, &mut encoder, view, &view.model);
        {
            let mut pass = color_pass(&mut encoder, &color_view, &depth_view, true, true, "gsplat");
            if let Some((sky, exposure)) = sky {
                sky.draw(queue, &mut pass, &view.view, &view.projection, exposure);
            }
            self.draw_color(&mut pass, pipelines);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(view.height),
                },
            },
            extent,
        );
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll device");
        rx.recv().expect("map callback").expect("map readback buffer");

        let mapped = slice.get_mapped_range().expect("mapped range");
        let row_bytes = (view.width * 4) as usize;
        let mut rgba = Vec::with_capacity(row_bytes * view.height as usize);
        for row in mapped.chunks_exact(bytes_per_row as usize) {
            rgba.extend_from_slice(&row[..row_bytes]);
        }
        drop(mapped);
        readback.unmap();
        rgba
    }
}

/// Per-axis 0.5th and 99.5th percentiles: a box that ignores stray splats.
fn robust_bounds(centres: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    if centres.is_empty() {
        return (lo, hi);
    }
    let mut values = vec![0.0f32; centres.len()];
    let (a, b) = (centres.len() / 200, centres.len() - 1 - centres.len() / 200);
    for axis in 0..3 {
        values.iter_mut().zip(centres).for_each(|(v, c)| *v = c[axis]);
        lo[axis] = *values.select_nth_unstable_by(a, f32::total_cmp).1;
        hi[axis] = *values.select_nth_unstable_by(b, f32::total_cmp).1;
    }
    (lo, hi)
}

/// A colour pass over `color`, tested against `depth`. `clear_color` clears the colour to black (else it loads);
/// `clear_depth` clears the depth to the far plane (else it loads what an earlier pass wrote).
pub fn color_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    clear_color: bool,
    clear_depth: bool,
    label: &str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear_color { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: if clear_depth { wgpu::LoadOp::Clear(1.0) } else { wgpu::LoadOp::Load },
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A depth-only pass that clears `depth` to the far plane, for `GsplatScene::draw_depth`.
pub fn depth_pass<'e>(encoder: &'e mut wgpu::CommandEncoder, depth: &wgpu::TextureView, label: &str) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// Named access to the scene's uniform and storage buffers.
struct Buffers<'a>(&'a BTreeMap<String, wgpu::Buffer>);

impl Buffers<'_> {
    /// Writes `bytes` to the buffer for `name`. Names missing from the binding table are ignored.
    fn set(&self, queue: &wgpu::Queue, name: &str, bytes: &[u8]) {
        if let Some(buffer) = self.0.get(name) {
            queue.write_buffer(buffer, 0, bytes);
        }
    }
}

/// Six vertices per splat slot in an instance: two triangles over the quad corners in [-1, 1]².
/// Each vertex is `(x, y, slot)`, where `slot` becomes `vertex_position.z` (the offset within an instance).
fn quad_vertices() -> Vec<f32> {
    const CORNERS: [(f32, f32); 6] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let mut out = Vec::with_capacity((INSTANCE_SIZE as usize) * 6 * 3);
    for slot in 0..INSTANCE_SIZE {
        for (x, y) in CORNERS {
            out.extend_from_slice(&[x, y, slot as f32]);
        }
    }
    out
}

/// Writes `count` texels from slot `start` of a texture laid out `width` slots per row: the partial first row, the
/// full rows in one copy, then the partial last row.
fn write_slots(queue: &wgpu::Queue, texture: &wgpu::Texture, width: u32, start: u32, count: u32, texels: &[u8]) {
    let write = |x: u32, y: u32, w: u32, h: u32, from: usize| {
        let bytes = (w * h * 4) as usize;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            &texels[from..from + bytes],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    };
    let (mut slot, end, mut from) = (start, start + count, 0usize);
    if slot % width != 0 {
        let w = (width - slot % width).min(end - slot);
        write(slot % width, slot / width, w, 1, from);
        slot += w;
        from += w as usize * 4;
    }
    let full = (end - slot) / width;
    if full > 0 {
        write(0, slot / width, width, full, from);
        slot += full * width;
        from += (full * width) as usize * 4;
    }
    if slot < end {
        write(0, slot / width, end - slot, 1, from);
    }
}

/// Creates a texture and, with `data`, fills it.
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    data: Option<&[u8]>,
) -> wgpu::Texture {
    let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let Some(data) = data else { return texture };
    let bytes_per_texel = format.block_copy_size(None).expect("uncompressed format");
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * bytes_per_texel),
            rows_per_image: Some(height),
        },
        size,
    );
    texture
}

fn f32s(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
