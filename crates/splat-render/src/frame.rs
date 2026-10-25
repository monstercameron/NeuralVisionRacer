//! The GPU frame: device, offscreen colour and depth, the car and level splats and the sky, presented to a surface.
//!
//! The car and the level are sorted separately (the car moves; the level is several chunks), so blending alone
//! cannot put one behind the other. Each frame:
//!
//! 1. the sky clears the colour;
//! 2. depth prepasses write the solid cores of the level's splats into one depth buffer and the car's into another;
//! 3. the level draws, sorted as one set, tested against the car's depth, so the street behind the car is hidden by
//!    it (its chunks are merged first: they overlap in space, so drawing them one after another would mis-order them);
//! 4. the car draws over that, tested against the level's depth, so walls and posts in front of it hide it.
//!
//! Each object is only tested against the other one, never itself, so its own blending is unchanged.
//!
//! Every frame is rendered into an offscreen `COLOR_FORMAT` target and then copied to the surface, so one frame
//! serves the window, the browser canvas and `read_rgba`. Initialisation is `async`, so the same code runs under
//! `pollster` on the desktop and under `wasm_bindgen_futures` in the browser.

use crate::camera::{self, Mat4};
use crate::env::{Cubemap, Sky};
use crate::scene::{self, GsplatScene, Pipelines, View};
use crate::sog::Bundle;
use crate::{COLOR_FORMAT, DEPTH_FORMAT};

/// How opaque a level splat must be, at a pixel, to hide the car behind it.
const LEVEL_DEPTH_ALPHA: f32 = 0.5;
/// How opaque a car splat must be to hide the level behind it. Higher than the level's, so the car's soft edges
/// still show the street through them.
const CAR_DEPTH_ALPHA: f32 = 0.8;

struct Presenter {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    blit: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    /// The colour target bound for the blit; rebuilt when the targets are.
    blit_group: Option<wgpu::BindGroup>,
}

pub struct Frame {
    device: wgpu::Device,
    queue: wgpu::Queue,
    presenter: Option<Presenter>,
    width: u32,
    height: u32,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    /// The scene after the screen effects (`post`), with the HUD on top: what is presented and read back.
    post_target: wgpu::Texture,
    /// The scene after depth of field, which the screen effects read.
    dof_view: wgpu::TextureView,
    dof: DofLayer,
    /// Distance from the camera to where the streamed level stops being sharp (`stream_level`, `sharp_distance`).
    auto_focus_m: Option<f32>,
    post_view: wgpu::TextureView,
    post: PostLayer,
    /// Depth of the level's solid splats; the car is tested against it.
    depth_level: wgpu::TextureView,
    /// Depth of the car's solid splats; the level is tested against it.
    depth_car: wgpu::TextureView,
    pipelines: Pipelines,
    car: Option<GsplatScene>,
    level: Vec<GsplatScene>,
    level_model: Mat4,
    /// The hole cut through the level so the car stays in view; `None` turns it off.
    cutout: Option<crate::sort::Cutout>,
    /// World point level detail is centred on (`set_lod_focus`); `None` is the camera.
    lod_focus: Option<[f32; 3]>,
    /// The car's shadow on the level: strength (0..1) and the world direction towards the sun; `None` is off.
    shadow: Option<(f32, [f32; 3])>,
    sky: Option<Sky>,
    exposure: f32,
    frames_presented: u64,
    overlay: OverlayLayer,
    lines: LineLayer,
    /// The host's clock in milliseconds (`set_clock`), and the GPU time of the latest finished frame, as f64 bits.
    clock: Option<fn() -> f64>,
    gpu_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

/// The screen effects' settings (see `shaders/post.wgsl`). The default shows the scene unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PostSettings {
    /// Peripheral blur 0..1, starting at screen radius `blur_start` (0 centre, 1 edge) with streaks `blur_length` long.
    pub blur: f32,
    pub blur_start: f32,
    pub blur_length: f32,
    /// Colour fringing: the red/blue offset at the corners, in screen units.
    pub fringe: f32,
    /// Film grain strength.
    pub grain: f32,
    /// How much of the grade to apply (0..1), and its parts: S-curve contrast, saturation, vignette, and the tints
    /// added to shadows and highlights.
    pub grade: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub vignette: f32,
    pub shadow_tint: [f32; 3],
    pub highlight_tint: [f32; 3],
    /// Seconds, for the grain.
    pub time: f32,
    /// Depth of field 0..1 (`shaders/dof.wgsl`): sharp out to `dof_focus_m` from the camera (0: follows the streamed
    /// level, softening from half way through its LOD 1 band so coarser tiles are always soft), softening over `dof_range_m` to `dof_radius_px` of blur at a
    /// 1080-pixel-tall frame.
    pub dof: f32,
    pub dof_focus_m: f32,
    pub dof_range_m: f32,
    pub dof_radius_px: f32,
}

/// The screen-effects pass (`shaders/post.wgsl`): peripheral blur, colour fringing, the grade and grain.
struct PostLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    /// Binds the scene colour target; rebuilt when the targets are.
    group: wgpu::BindGroup,
    settings: PostSettings,
}

/// The depth-of-field pass (`shaders/dof.wgsl`): the scene and both depth prepasses in, a softened scene out.
struct DofLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    /// Binds the scene colour and depth targets; rebuilt when the targets are.
    group: Option<wgpu::BindGroup>,
}

/// A premultiplied RGBA image drawn over the finished frame (the dashboard HUD), and where.
struct OverlayLayer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The texture, its bind group and the rectangle `[x, y, width, height]` in pixels; `None` hides the overlay.
    current: Option<(wgpu::Texture, wgpu::BindGroup, [u32; 4])>,
}

/// Debug lines over the finished frame: the streamed level's octree leaves (in the level's splat space) and the
/// collision around the car (world space).
struct LineLayer {
    pipeline: wgpu::RenderPipeline,
    lod: LineBatch,
    world: LineBatch,
}

/// One set of lines with its own transform.
struct LineBatch {
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// Vertices (position, colour) and how many to draw; `None` draws nothing.
    vertices: Option<(wgpu::Buffer, u32)>,
    /// What the vertices were built from, so unchanged lines are not uploaded again.
    key: Option<u64>,
}

/// Collision debug colours: the floor and the walls.
pub const FLOOR_COLOUR: [f32; 4] = [0.1, 0.85, 1.0, 0.4];
pub const WALL_COLOUR: [f32; 4] = [1.0, 0.25, 0.9, 0.22];

/// Debug colours for levels of detail: LOD 0 green, 1 yellow, 2 orange, 3 red, 4 and coarser magenta.
pub const LOD_COLOURS: [[f32; 3]; 5] = [[0.1, 1.0, 0.2], [1.0, 0.95, 0.1], [1.0, 0.5, 0.05], [1.0, 0.1, 0.1], [1.0, 0.2, 1.0]];

impl Frame {
    /// Requests a device and builds the frame. `surface` is `None` for offscreen-only rendering. `bundle` is the
    /// car, drawn with `View::model`, or `None`. Level chunks are added with `add_level_chunk`.
    pub async fn new(
        instance: &wgpu::Instance,
        surface: Option<wgpu::Surface<'static>>,
        size: (u32, u32),
        vsync: bool,
        bundle: Option<&Bundle>,
    ) -> Result<Self, String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: surface.as_ref(), ..Default::default() })
            .await
            .map_err(|e| format!("request adapter: {e}"))?;
        // A taller texture limit than WebGPU's default (8192) when the GPU has it: a streaming pool's SH centroid
        // texture holds one palette per 1024 rows, so 16384 rows fit 15 chunks' view-dependent colour, not 7.
        // Larger buffers too where the GPU allows them, for big streaming pools (12 bytes of centre per splat).
        let adapter_limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_texture_dimension_2d: adapter_limits.max_texture_dimension_2d.clamp(8192, 16384),
            max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size.clamp(128 << 20, 1 << 30),
            max_buffer_size: adapter_limits.max_buffer_size.clamp(256 << 20, 1 << 30),
            ..wgpu::Limits::default()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor { required_limits, ..Default::default() })
            .await
            .map_err(|e| format!("request device: {e}"))?;
        let (width, height) = (size.0.max(1), size.1.max(1));
        let mut presenter = surface.map(|surface| configure_presenter(surface, &adapter, &device, width, height, vsync));
        let (color, color_view, depth_level, depth_car) = targets(&device, width, height);
        // The view keeps its texture alive.
        let (_, dof_view) = post_target(&device, width, height);
        let (post_target, post_view) = post_target(&device, width, height);
        let dof = DofLayer::new(&device);
        let dof_group = dof.bind(&device, &color_view, &depth_level, &depth_car);
        let dof = DofLayer { group: Some(dof_group), ..dof };
        let post = PostLayer::new(&device, &dof_view);
        if let Some(presenter) = presenter.as_mut() {
            presenter.blit_group = Some(blit_group(&device, presenter, &post_view));
        }
        let pipelines = Pipelines::new(&device, COLOR_FORMAT).map_err(|e| format!("build pipelines: {e}"))?;
        let car = match bundle {
            Some(bundle) => Some(scene::build(&device, &queue, &pipelines, bundle, CAR_DEPTH_ALPHA).map_err(|e| format!("build car: {e}"))?),
            None => None,
        };
        let overlay = OverlayLayer::new(&device);
        let lines = LineLayer::new(&device);
        Ok(Self {
            device,
            queue,
            presenter,
            width,
            height,
            color,
            color_view,
            post_target,
            dof_view,
            dof,
            auto_focus_m: None,
            post_view,
            post,
            depth_level,
            depth_car,
            pipelines,
            car,
            level: Vec::new(),
            level_model: camera::IDENTITY,
            cutout: None,
            lod_focus: None,
            shadow: None,
            sky: None,
            exposure: 1.0,
            frames_presented: 0,
            overlay,
            lines,
            clock: None,
            gpu_ms: Default::default(),
        })
    }

    /// Replaces the level with these chunks, merged into one splat set so a single sort orders all of them. If they
    /// cannot be merged (too many SH centroids for one texture), each chunk is drawn on its own, ordered by distance,
    /// which can mis-order splats where chunks overlap; the returned message says so. The level draws with
    /// `set_level_model`'s matrix.
    pub fn set_level(&mut self, chunks: &[Bundle]) -> Result<Option<String>, String> {
        self.level.clear();
        if chunks.is_empty() {
            return Ok(None);
        }
        let build = |bundle: &Bundle| scene::build(&self.device, &self.queue, &self.pipelines, bundle, LEVEL_DEPTH_ALPHA).map_err(|e| format!("build level: {e}"));
        match crate::sog::merge(chunks) {
            Ok(merged) => {
                let scene = build(&merged)?;
                self.level.push(scene);
                Ok(None)
            }
            Err(error) => {
                let scenes = chunks.iter().map(build).collect::<Result<Vec<_>, _>>()?;
                self.level = scenes;
                Ok(Some(format!("level drawn as {} separate chunks ({error}); lower level.lod for correct ordering", chunks.len())))
            }
        }
    }

    /// Casts the car's shadow on the level from a sun in the world direction `sun`, darkening by up to `strength`
    /// (0..1); `None` turns it off.
    pub fn set_shadow(&mut self, shadow: Option<(f32, [f32; 3])>) {
        self.shadow = shadow;
    }

    /// Cuts a hole through the level around `cutout`'s target (the car) from now on, or stops with `None`.
    pub fn set_cutout(&mut self, cutout: Option<crate::sort::Cutout>) {
        self.cutout = cutout;
    }

    /// Draws the streamed level's resident octree leaves as wireframe boxes coloured by their level of detail
    /// (`LOD_COLOURS`), over everything; `None` turns the layer off. A leaf still waiting for the detail it wants is
    /// drawn brighter, and leaves out of the camera's view are dimmed.
    pub fn set_lod_debug(&mut self, stream: Option<&crate::stream::LevelStream>) {
        let Some(stream) = stream else {
            self.lines.lod.vertices = None;
            return;
        };
        const EDGES: [(usize, usize); 12] = [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)];
        let mut data: Vec<f32> = Vec::new();
        for leaf in stream.debug_leaves() {
            let base = LOD_COLOURS[leaf.lod.min(LOD_COLOURS.len() - 1)];
            let alpha = if !leaf.visible { 0.25 } else if leaf.pending { 1.0 } else { 0.7 };
            let colour = if leaf.pending { base.map(|c| (c + 0.35).min(1.0)) } else { base };
            let corner = |k: usize| [0, 1, 2].map(|i| if k >> i & 1 == 1 { leaf.max[i] } else { leaf.min[i] });
            for (a, b) in EDGES {
                for k in [a, b] {
                    data.extend_from_slice(&corner(k));
                    data.extend_from_slice(&[colour[0], colour[1], colour[2], alpha]);
                }
            }
        }
        upload_lines(&self.device, &self.queue, &mut self.lines.lod, &data);
    }

    /// Draws world-space line segments (the collision around the car: `FLOOR_COLOUR`, `WALL_COLOUR`) over
    /// everything; `None` turns them off. `key` names what the lines show (the collision's generation): lines are
    /// built and uploaded only when it changes, and `lines` is only called then.
    pub fn set_world_debug(&mut self, key: Option<u64>, lines: impl FnOnce() -> Vec<([f32; 3], [f32; 3], [f32; 4])>) {
        let Some(key) = key else {
            self.lines.world.vertices = None;
            self.lines.world.key = None;
            return;
        };
        if self.lines.world.key == Some(key) {
            return;
        }
        let mut data: Vec<f32> = Vec::new();
        for (a, b, colour) in lines() {
            for p in [a, b] {
                data.extend_from_slice(&p);
                data.extend_from_slice(&colour);
            }
        }
        upload_lines(&self.device, &self.queue, &mut self.lines.world, &data);
        self.lines.world.key = Some(key);
    }

    /// Debug colouring of the streamed level's splats: 0 normal, 1 in their level of detail's colour
    /// (`LOD_COLOURS`), to see in the picture itself where coarse detail shows, 2 by distance from the camera (red =
    /// metres / 128).
    pub fn set_debug_colour(&mut self, mode: u32) {
        for chunk in &self.level {
            chunk.set_debug_colour(&self.queue, mode);
        }
    }

    /// Gives the frame a clock (milliseconds, any steady origin) so it can time its GPU work (`gpu_ms`).
    pub fn set_clock(&mut self, clock: fn() -> f64) {
        self.clock = Some(clock);
    }

    /// Milliseconds the GPU took over the latest frame that has finished (from submitting it until the queue said
    /// it was done; includes any wait behind earlier frames). 0 before the first, or without a clock.
    pub fn gpu_ms(&self) -> f64 {
        f64::from_bits(self.gpu_ms.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Centres the streamed level's detail on a world point (the car) instead of the camera: a chase camera swings
    /// round the car, and detail measured from the camera would follow it round, swapping leaves all the way.
    /// Leaves still go coarser out of view, by the camera's frustum.
    pub fn set_lod_focus(&mut self, focus: Option<[f32; 3]>) {
        self.lod_focus = focus;
    }

    /// Sets the screen effects (blur, fringing, grain and grade). The default shows the scene unchanged.
    pub fn set_post(&mut self, settings: PostSettings) {
        self.post.settings = settings;
    }

    /// Sets the overlay drawn over every frame from now on: `rgba` is `width × height` premultiplied pixels, rows
    /// top first, placed at (`x`, `y`). `None` removes it.
    pub fn set_overlay(&mut self, overlay: Option<(u32, u32, u32, u32, &[u8])>) {
        let Some((x, y, width, height, rgba)) = overlay.filter(|o| o.2 > 0 && o.3 > 0) else {
            self.overlay.current = None;
            return;
        };
        let reuse = self.overlay.current.as_ref().is_some_and(|(t, _, _)| t.width() == width && t.height() == height);
        if !reuse {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("overlay"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("overlay"),
                layout: &self.overlay.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.overlay.sampler) },
                ],
            });
            self.overlay.current = Some((texture, group, [x, y, width, height]));
        }
        if let Some((texture, _, rect)) = self.overlay.current.as_mut() {
            *rect = [x, y, width, height];
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
        }
    }

    /// Switches the level to streaming: an empty GPU pool sized for `stream`, filled as `stream_level` runs.
    pub fn start_level_stream(&mut self, stream: &mut crate::stream::LevelStream) -> Result<(), String> {
        let pool = scene::pool(&self.device, &self.queue, &self.pipelines, stream.shared_meta(), stream.settings.capacity, LEVEL_DEPTH_ALPHA)
            .map_err(|e| format!("build level pool: {e}"))?;
        stream.init_pool(&pool, &self.queue);
        self.level = vec![pool];
        Ok(())
    }

    /// One streaming step for the camera in `view` at `now_s` seconds (any steady clock; crossfades and the keep
    /// delay run on it): picks each leaf's LOD and uploads what has arrived (see `stream`). Returns the splats
    /// uploaded. The host fetches `stream.requests()` and hands them to `insert`.
    pub fn stream_level(&mut self, stream: &mut crate::stream::LevelStream, view: &View, now_s: f32) -> u32 {
        let Some(pool) = self.level.first() else { return 0 };
        let Some(camera) = camera::inverse(&view.view) else { return 0 };
        // Camera position and forward (it looks down its −z) in world space, then in the level's splat space.
        let to_level = camera::inverse(&self.level_model).unwrap_or(camera::IDENTITY);
        let eye = camera::transform_point(&to_level, [camera[12], camera[13], camera[14]]);
        let ahead = camera::transform_point(&to_level, [camera[12] - camera[8], camera[13] - camera[9], camera[14] - camera[10]]);
        let forward = [ahead[0] - eye[0], ahead[1] - eye[1], ahead[2] - eye[2]];
        let eye = self.lod_focus.map_or(eye, |focus| camera::transform_point(&to_level, focus));
        // The view frustum in the level's splat space, where the leaves' boxes are.
        let clip = camera::mul(&view.projection, &camera::mul(&view.view, &self.level_model));
        // Screen-space error at a 1080-pixel-tall view, whatever the window: projection[5] = 1 / tan(half field of view).
        stream.set_focal_px(view.projection[5] * 540.0);
        stream.select(eye, forward, Some(&clip), now_s);
        let camera_to_centre = camera::transform_point(&to_level, [camera[12], camera[13], camera[14]]);
        let gap = (0..3).map(|i| (camera_to_centre[i] - eye[i]).powi(2)).sum::<f32>().sqrt();
        self.auto_focus_m = Some(gap + stream.sharp_distance());
        stream.apply(pool, &self.queue)
    }

    /// Removes every level chunk.
    pub fn clear_level(&mut self) {
        self.level.clear();
    }

    /// The level's splat-to-world matrix. Identity by default: the level's splats and its collision voxels share
    /// one space.
    pub fn set_level_model(&mut self, model: Mat4) {
        self.level_model = model;
    }

    /// Splat sets the level is drawn as: 1 when merged, 0 without a level.
    pub fn level_sets(&self) -> usize {
        self.level.len()
    }

    /// Builds a sky from a cubemap for this frame's colour format.
    pub fn build_sky(&self, cubemap: &Cubemap) -> Sky {
        Sky::new(&self.device, &self.queue, cubemap, COLOR_FORMAT)
    }

    /// Sets the sky drawn behind the splats (`None` for black), and its linear exposure.
    pub fn set_sky(&mut self, sky: Option<Sky>, exposure: f32) {
        self.sky = sky;
        self.exposure = exposure;
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Resizes the offscreen targets and, with a surface, the swapchain.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width == self.width && height == self.height) {
            return;
        }
        self.width = width;
        self.height = height;
        if let Some(presenter) = self.presenter.as_mut() {
            presenter.config.width = width;
            presenter.config.height = height;
            presenter.surface.configure(&self.device, &presenter.config);
        }
        (self.color, self.color_view, self.depth_level, self.depth_car) = targets(&self.device, width, height);
        (self.post_target, self.post_view) = post_target(&self.device, width, height);
        (_, self.dof_view) = post_target(&self.device, width, height);
        self.dof.group = Some(self.dof.bind(&self.device, &self.color_view, &self.depth_level, &self.depth_car));
        self.post.group = self.post.bind(&self.device, &self.dof_view);
        if let Some(presenter) = self.presenter.as_mut() {
            presenter.blit_group = Some(blit_group(&self.device, presenter, &self.post_view));
        }
    }

    /// Renders `view` into the offscreen target and, with a surface, presents it.
    pub fn render(&mut self, view: &View) {
        let frame = match self.presenter.as_ref() {
            Some(presenter) => match presenter.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
                other => {
                    let presenter = self.presenter.as_ref().expect("checked above");
                    eprintln!("splat-racer: surface texture unavailable: {other:?}; reconfiguring");
                    presenter.surface.configure(&self.device, &presenter.config);
                    return;
                }
            },
            None => None,
        };
        self.frames_presented += 1;
        if self.frames_presented <= 3 || self.frames_presented % 300 == 0 {
            eprintln!("splat-racer: frame {} presented to the surface", self.frames_presented);
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.draw(&mut encoder, view);
        if let (Some(frame), Some(presenter)) = (frame.as_ref(), self.presenter.as_ref()) {
            let target = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(frame.texture.format().add_srgb_suffix()),
                ..Default::default()
            });
            blit(presenter, &mut encoder, &target);
        }
        self.queue.submit(Some(encoder.finish()));
        // Time the GPU work: from submitting until the queue reports it done (for frame pacing).
        if let Some(clock) = self.clock {
            let (submitted, gpu_ms) = (clock(), self.gpu_ms.clone());
            self.queue.on_submitted_work_done(move || {
                gpu_ms.store((clock() - submitted).to_bits(), std::sync::atomic::Ordering::Relaxed);
            });
            // Natively, finished-work callbacks run when the device is polled; this does not wait.
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
        // Dropping the surface texture presents it.
        drop(frame);
    }

    /// Blocks until every submitted frame has finished on the GPU. For timing; presenting never needs it.
    pub fn wait_idle(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// Renders `view` offscreen and returns the RGBA8 pixels, top row first, sRGB-encoded.
    pub async fn read_rgba(&mut self, view: &View) -> Vec<u8> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.draw(&mut encoder, view);
        let bytes_per_row = (self.width * 4).next_multiple_of(256);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame readback"),
            size: u64::from(bytes_per_row) * u64::from(self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.post_target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(self.height) },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = futures_channel::oneshot();
        slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result));
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let _ = rx.await;
        let mapped = slice.get_mapped_range().expect("mapped range");
        let row = (self.width * 4) as usize;
        let mut pixels = Vec::with_capacity(row * self.height as usize);
        for line in mapped.chunks_exact(bytes_per_row as usize) {
            pixels.extend_from_slice(&line[..row]);
        }
        drop(mapped);
        readback.unmap();
        pixels
    }

    fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &View) {
        let queue = &self.queue;
        // The car's shadow: level splats are tested against the car's box (see the `carShadowVS` shader chunk).
        let off = f32_bytes(&[0.0; 16]);
        let shadow = self.shadow.filter(|s| s.0 > 0.0).zip(self.car.as_ref()).and_then(|((strength, sun), car)| {
            let to_box = camera::mul(&unit_box(car.bounds), &camera::inverse(&view.model)?);
            let sun_box = [0, 1, 2].map(|r| to_box[r] * sun[0] + to_box[4 + r] * sun[1] + to_box[8 + r] * sun[2]);
            Some((camera::mul(&to_box, &self.level_model), sun_box, strength))
        });
        for chunk in &self.level {
            match shadow {
                Some((mut to_box, sun, strength)) => {
                    // The fourth row (unused by the affine map) carries the sun direction and the strength.
                    (to_box[3], to_box[7], to_box[11], to_box[15]) = (sun[0], sun[1], sun[2], strength);
                    chunk.set_uniform(queue, "shadow_box", &f32_bytes(&to_box));
                }
                None => chunk.set_uniform(queue, "shadow_box", &off),
            }
            chunk.prepare_with_cutout(queue, encoder, view, &self.level_model, self.cutout.as_ref());
        }
        if let Some(car) = &self.car {
            car.set_uniform(queue, "shadow_box", &off);
        }
        if let Some(car) = &self.car {
            car.prepare(queue, encoder, view, &view.model);
        }

        // 1. The sky, over a cleared colour. It tests against a cleared depth, so it covers the whole frame.
        {
            let mut pass = scene::color_pass(encoder, &self.color_view, &self.depth_car, true, true, "sky");
            if let Some(sky) = &self.sky {
                sky.draw(queue, &mut pass, &view.view, &view.projection, self.exposure);
            }
        }
        // 2. The depth prepasses. Each clears its buffer, so an object that is absent hides nothing. The level's depth
        //    only matters where the car is drawn, so its prepass is clipped to the car's rectangle on screen and skipped
        //    when the car is off screen; that saves most of a second full draw of the level.
        {
            //    The depth of field needs the level's depth everywhere, so with it on the prepass covers the frame.
            let mut pass = scene::depth_pass(encoder, &self.depth_level, "level depth");
            let rect = if self.post.settings.dof > 0.0 {
                Some([0, 0, self.width, self.height])
            } else {
                self.car.as_ref().and_then(|car| screen_rect(car.bounds, &view.model, view))
            };
            if let Some([x, y, w, h]) = rect {
                pass.set_scissor_rect(x, y, w, h);
                for chunk in &self.level {
                    chunk.draw_depth(&mut pass, &self.pipelines);
                }
            }
        }
        {
            let mut pass = scene::depth_pass(encoder, &self.depth_car, "car depth");
            if let Some(car) = &self.car {
                car.draw_depth(&mut pass, &self.pipelines);
            }
        }
        // 3. The level against the car's depth. Normally one merged set; unmerged chunks go farthest first.
        if !self.level.is_empty() {
            let eye = camera::inverse(&view.view).map_or([0.0; 3], |m| [m[12], m[13], m[14]]);
            let distance = |chunk: &GsplatScene| {
                let c = camera::transform_point(&self.level_model, chunk.centre);
                (0..3).map(|i| (c[i] - eye[i]).powi(2)).sum::<f32>()
            };
            let mut order: Vec<&GsplatScene> = self.level.iter().collect();
            order.sort_by(|a, b| distance(b).total_cmp(&distance(a)));
            let mut pass = scene::color_pass(encoder, &self.color_view, &self.depth_car, false, false, "level");
            for chunk in order {
                chunk.draw_color(&mut pass, &self.pipelines);
            }
        }
        // 4. The car, against the level's depth.
        if let Some(car) = &self.car {
            let mut pass = scene::color_pass(encoder, &self.color_view, &self.depth_level, false, false, "car");
            car.draw_color(&mut pass, &self.pipelines);
        }
        // 5. Depth of field, from the scene into its own target.
        {
            let s = &self.post.settings;
            let focus = if s.dof_focus_m > 0.0 { s.dof_focus_m } else { self.auto_focus_m.unwrap_or(30.0) };
            let radius = s.dof_radius_px * self.height as f32 / 1080.0;
            let p = &view.projection;
            let uniform = f32_bytes(&[s.dof.clamp(0.0, 1.0), focus, s.dof_range_m, radius, p[10], p[14], 1.0 / self.width as f32, 1.0 / self.height as f32]);
            queue.write_buffer(&self.dof.uniform, 0, &uniform);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("depth of field"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.dof_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.dof.pipeline);
            pass.set_bind_group(0, self.dof.group.as_ref().expect("bound at creation"), &[]);
            pass.draw(0..3, 0..1);
        }
        // 6. The screen effects, from the scene into the post target.
        {
            let p = &self.post;
            let s = &p.settings;
            let (w, h) = (self.width as f32, self.height as f32);
            let uniform = f32_bytes(&[
                s.blur, s.fringe, s.grain, s.grade,
                s.time, 1.0 / w, 1.0 / h, 0.0,
                s.contrast, s.saturation, s.vignette, s.blur_start,
                s.shadow_tint[0], s.shadow_tint[1], s.shadow_tint[2], s.blur_length,
                s.highlight_tint[0], s.highlight_tint[1], s.highlight_tint[2], 0.0,
            ]);
            queue.write_buffer(&p.uniform, 0, &uniform);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.post_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&p.pipeline);
            pass.set_bind_group(0, &p.group, &[]);
            pass.draw(0..3, 0..1);
        }
        // 7. Debug lines (the level's octree leaves, the collision), over the finished picture.
        let level_clip = camera::mul(&view.projection, &camera::mul(&view.view, &self.level_model));
        let world_clip = camera::mul(&view.projection, &view.view);
        for (batch, clip) in [(&self.lines.lod, level_clip), (&self.lines.world, world_clip)] {
            let Some((buffer, count)) = &batch.vertices else { continue };
            self.queue.write_buffer(&batch.uniform, 0, &f32_bytes(&clip));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("debug lines"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.post_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.lines.pipeline);
            pass.set_bind_group(0, &batch.group, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..*count, 0..1);
        }
        // 8. The overlay (the HUD), over everything, clipped to the frame.
        if let Some((_, group, [x, y, w, h])) = &self.overlay.current {
            let (x, y) = ((*x).min(self.width.saturating_sub(1)), (*y).min(self.height.saturating_sub(1)));
            let (w, h) = ((*w).min(self.width - x), (*h).min(self.height - y));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.post_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // The triangle covers the viewport; the texture is mapped to the overlay's own size, then clipped.
            let full = &self.overlay.current.as_ref().expect("checked").2;
            pass.set_viewport(full[0] as f32, full[1] as f32, full[2] as f32, full[3] as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w.max(1), h.max(1));
            pass.set_pipeline(&self.overlay.pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// The pixel rectangle `[x, y, width, height]` covering the box `bounds` (in model space) under `model` and `view`,
/// with a margin for splats that reach past their centres. `None` when the box is entirely off screen. A box that
/// reaches behind the camera covers the whole frame.
fn screen_rect(bounds: ([f32; 3], [f32; 3]), model: &Mat4, view: &View) -> Option<[u32; 4]> {
    let clip = camera::mul(&view.projection, &camera::mul(&view.view, model));
    let (lo, hi) = bounds;
    let full = Some([0, 0, view.width, view.height]);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for corner in 0..8 {
        let p = [if corner & 1 == 0 { lo[0] } else { hi[0] }, if corner & 2 == 0 { lo[1] } else { hi[1] }, if corner & 4 == 0 { lo[2] } else { hi[2] }];
        let c = [0, 1, 2, 3].map(|r| clip[r] * p[0] + clip[4 + r] * p[1] + clip[8 + r] * p[2] + clip[12 + r]);
        if c[3] <= 1e-4 {
            return full;
        }
        let (nx, ny) = (c[0] / c[3], c[1] / c[3]);
        min_x = min_x.min(nx);
        max_x = max_x.max(nx);
        min_y = min_y.min(ny);
        max_y = max_y.max(ny);
    }
    // NDC to pixels (y down), with a 10% margin of the rectangle's size and at least 16 px.
    let (w, h) = (view.width as f32, view.height as f32);
    let to_px = |nx: f32, ny: f32| ((nx + 1.0) * 0.5 * w, (1.0 - ny) * 0.5 * h);
    let (x0, y1) = to_px(min_x, min_y);
    let (x1, y0) = to_px(max_x, max_y);
    let margin = ((x1 - x0).max(y1 - y0) * 0.1).max(16.0);
    let (x0, y0, x1, y1) = ((x0 - margin).max(0.0), (y0 - margin).max(0.0), (x1 + margin).min(w), (y1 + margin).min(h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some([x0 as u32, y0 as u32, (x1 - x0).ceil() as u32, (y1 - y0).ceil() as u32].map(|v| v.max(0)))
        .map(|[x, y, rw, rh]| [x, y, rw.min(view.width - x).max(1), rh.min(view.height - y).max(1)])
}

impl PostLayer {
    fn new(device: &wgpu::Device, scene: &wgpu::TextureView) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: COLOR_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut post = Self { pipeline, layout, sampler, uniform, group: Self::placeholder(device), settings: PostSettings::default() };
        post.group = post.bind(device, scene);
        post
    }

    /// An empty bind group to hold the slot until `bind` runs.
    fn placeholder(device: &wgpu::Device) -> wgpu::BindGroup {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("post placeholder"), entries: &[] });
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("post placeholder"), layout: &layout, entries: &[] })
    }

    fn bind(&self, device: &wgpu::Device, scene: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(scene) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
            ],
        })
    }
}

/// The post target: what the screen effects write, the HUD draws on, and the frame presents and reads back.
fn post_target(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("post target"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

impl DofLayer {
    fn new(device: &wgpu::Device) -> Self {
        let texture = |binding: u32, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth of field"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Float { filterable: true }),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture(2, wgpu::TextureSampleType::Depth),
                texture(3, wgpu::TextureSampleType::Depth),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth of field"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/dof.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth of field"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth of field"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: COLOR_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("depth of field"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth of field"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, sampler, uniform, group: None }
    }

    fn bind(&self, device: &wgpu::Device, scene: &wgpu::TextureView, depth_level: &wgpu::TextureView, depth_car: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("depth of field"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(scene) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(depth_level) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(depth_car) },
                wgpu::BindGroupEntry { binding: 4, resource: self.uniform.as_entire_binding() },
            ],
        })
    }
}

impl OverlayLayer {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/overlay.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState { color: premultiplied, alpha: premultiplied }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipeline, layout, sampler, current: None }
    }
}

impl LineLayer {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug lines"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug lines"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/debug_lines.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug lines"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug lines"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 7 * 4,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState { color: premultiplied, alpha: premultiplied }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let (lod, world) = (LineBatch::new(device, &layout), LineBatch::new(device, &layout));
        Self { pipeline, lod, world }
    }
}

impl LineBatch {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug lines"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("debug lines"),
            layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        Self { uniform, group, vertices: None, key: None }
    }
}

/// Writes line vertices (position, colour: 7 floats each) into a batch, growing its buffer as needed.
fn upload_lines(device: &wgpu::Device, queue: &wgpu::Queue, batch: &mut LineBatch, data: &[f32]) {
    let count = (data.len() / 7) as u32;
    if count == 0 {
        batch.vertices = None;
        return;
    }
    let bytes = f32_bytes(data);
    if !batch.vertices.as_ref().is_some_and(|(b, _)| b.size() >= bytes.len() as u64) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug lines"),
            size: (bytes.len() as u64).next_power_of_two(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        batch.vertices = Some((buffer, 0));
    }
    if let Some((buffer, n)) = batch.vertices.as_mut() {
        queue.write_buffer(buffer, 0, &bytes);
        *n = count;
    }
}

/// The affine map from a box's space (`bounds` in model units) to [-1, 1] on every axis.
fn unit_box(bounds: ([f32; 3], [f32; 3])) -> Mat4 {
    let (lo, hi) = bounds;
    let s = [0, 1, 2].map(|i| 2.0 / (hi[i] - lo[i]).max(1e-4));
    let c = [0, 1, 2].map(|i| (lo[i] + hi[i]) * 0.5);
    [s[0], 0.0, 0.0, 0.0, 0.0, s[1], 0.0, 0.0, 0.0, 0.0, s[2], 0.0, -c[0] * s[0], -c[1] * s[1], -c[2] * s[2], 1.0]
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// The model matrix for a rigid body: `translate(position) · rotate(rotation) · translate(-centroid) · base`.
pub fn body_model(position: [f32; 3], rotation_xyzw: [f32; 4], centroid: [f32; 3], base: Mat4) -> Mat4 {
    let back = camera::translate([-centroid[0], -centroid[1], -centroid[2]]);
    let body = camera::mul(&camera::quat_to_mat4(rotation_xyzw), &back);
    camera::mul(&camera::translate(position), &camera::mul(&body, &base))
}

/// Oneshot channel for the readback callback, without an extra dependency. The callback must be `Send`, so the
/// shared slot is behind a `Mutex`.
mod futures_channel {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    type Shared<T> = Arc<Mutex<(Option<T>, Option<Waker>)>>;

    pub struct Sender<T>(Shared<T>);
    pub struct Receiver<T>(Shared<T>);

    pub fn oneshot<T>() -> (Sender<T>, Receiver<T>) {
        let shared = Arc::new(Mutex::new((None, None)));
        (Sender(shared.clone()), Receiver(shared))
    }

    impl<T> Sender<T> {
        pub fn send(self, value: T) {
            let mut shared = self.0.lock().expect("oneshot lock");
            shared.0 = Some(value);
            if let Some(waker) = shared.1.take() {
                waker.wake();
            }
        }
    }

    impl<T> Future for Receiver<T> {
        type Output = Option<T>;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
            let mut shared = self.0.lock().expect("oneshot lock");
            match shared.0.take() {
                Some(value) => Poll::Ready(Some(value)),
                None => {
                    shared.1 = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }
}

fn configure_presenter(surface: wgpu::Surface<'static>, adapter: &wgpu::Adapter, device: &wgpu::Device, width: u32, height: u32, vsync: bool) -> Presenter {
    let capabilities = surface.get_capabilities(adapter);
    // The blit writes linear colour, so the surface must encode sRGB. A browser canvas offers only `bgra8unorm`, but
    // allows an sRGB view of it, so the frame is presented through that view.
    let base = capabilities.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(capabilities.formats[0]);
    let format = base.add_srgb_suffix();
    let view_formats = if format == base { vec![] } else { vec![format] };
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: base,
        color_space: Default::default(),
        width,
        height,
        present_mode: if vsync { wgpu::PresentMode::AutoVsync } else { wgpu::PresentMode::AutoNoVsync },
        alpha_mode: capabilities.alpha_modes[0],
        view_formats,
        desired_maximum_frame_latency: 2,
    };
    surface.configure(device, &config);

    let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("blit"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("blit"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blit.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("blit"),
        bind_group_layouts: &[Some(&blit_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("blit"),
        layout: Some(&layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    Presenter { surface, config, blit: pipeline, blit_layout, blit_group: None }
}

/// Binds the offscreen colour target for the blit.
fn blit_group(device: &wgpu::Device, presenter: &Presenter, source: &wgpu::TextureView) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("blit"),
        layout: &presenter.blit_layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) }],
    })
}

fn blit(presenter: &Presenter, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
    let Some(bind_group) = presenter.blit_group.as_ref() else { return };
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("blit to surface"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(&presenter.blit);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

/// The offscreen colour target and the two depth buffers (level, car), at `width × height`.
fn targets(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView, wgpu::TextureView, wgpu::TextureView) {
    let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame colour"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&Default::default());
    let depth = |label: &str| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                // Also sampled by the depth of field.
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    (color, color_view, depth("level depth"), depth("car depth"))
}
