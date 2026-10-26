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

#[cfg(test)]
#[path = "frame_clarity_tests.rs"]
mod clarity_tests;
#[cfg(test)]
#[path = "frame_portal_tests.rs"]
mod portal_tests;
#[cfg(test)]
#[path = "frame_grade_tests.rs"]
mod grade_tests;

#[cfg(test)]
#[path = "frame_perf_tests.rs"]
mod perf_tests;
#[cfg(test)]
#[path = "frame_dust_tests.rs"]
mod dust_tests;
#[cfg(test)]
#[path = "frame_tire_tests.rs"]
mod tire_tests;

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
    /// Offscreen post/HUD target for readback and the optional copy presentation path.
    post_target: wgpu::Texture,
    /// The scene after depth of field, which the screen effects read.
    dof_view: wgpu::TextureView,
    dof: DofLayer,
    /// Distance from the camera to where the streamed level stops being sharp (`stream_level`, `sharp_distance`).
    auto_focus_m: Option<f32>,
    post_view: wgpu::TextureView,
    post: PostLayer,
    post_raw: wgpu::BindGroup,
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
    shadow_softness:f32,
    car_light: Option<crate::surface::CarLight>,
    sky: Option<Sky>,
    /// Sky, level, car visibility; hidden scenes skip preparation, sorting and draws.
    visibility: [bool; 3],
    exposure: f32,
    frames_presented: u64,
    direct_present: std::cell::Cell<bool>,
    overlay: OverlayLayer,
    lines: LineLayer,
    tires: crate::tire::TireLayer,
    dust: crate::dust::DustLayer,
    /// The host's clock in milliseconds (`set_clock`), and the GPU time of the latest finished frame, as f64 bits.
    clock: Option<fn() -> f64>,
    gpu_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
    queue_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
    in_flight: std::sync::Arc<std::sync::atomic::AtomicU32>,
    gpu_timer: Option<crate::timing::GpuTimer>,
    adapter_info:wgpu::AdapterInfo,
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
    /// How much of the grade to apply (0..1), and its parts: luminance contrast, saturation, vignette, and the tints
    /// added to shadows and highlights.
    pub grade: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub vignette: f32,
    pub shadow_tint: [f32; 3],
    pub highlight_tint: [f32; 3],
    /// Film controls: exposure in stops; warm/cool and magenta/green balance (-1..1);
    /// highlight shoulder (0..1) and neutral display-space black lift (0..0.04).
    pub grade_exposure_ev: f32,
    pub grade_temperature: f32,
    pub grade_tint: f32,
    pub grade_shoulder: f32,
    pub grade_black_lift: f32,
    /// Seconds, for the grain.
    pub time: f32,
    /// Depth of field 0..1 (`shaders/dof.wgsl`): sharp out to `dof_focus_m` from the camera (0: follows the streamed
    /// level, softening from half way through its LOD 1 band so coarser tiles are always soft), softening over `dof_range_m` to `dof_radius_px` of blur at a
    /// 1080-pixel-tall frame.
    pub dof: f32,
    pub dof_focus_m: f32,
    pub dof_range_m: f32,
    pub dof_radius_px: f32,
    /// Edge-limited sharpening and wider local contrast, masked to the visible car.
    pub car_sharpen: f32,
    pub car_clarity: f32,
    /// Subtract this faint alpha contribution from car splats, preserving opaque cores.
    pub car_haze_cutoff: f32,
    /// Bevelled foreground cutout rim (0..1), with width in pixels at 1080p.
    pub portal_frame: f32,
    pub portal_width_px: f32,
}

/// The screen-effects pass (`shaders/post.wgsl`): peripheral blur, colour fringing, the grade and grain.
struct PostLayer {
    pipeline: wgpu::RenderPipeline,
    surface_pipeline: Option<wgpu::RenderPipeline>,
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
    surface_pipeline: Option<wgpu::RenderPipeline>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The texture, its bind group and the rectangle `[x, y, width, height]` in pixels; `None` hides the overlay.
    current: Option<(wgpu::Texture, wgpu::BindGroup, [u32; 4])>,
}

/// Debug lines over the finished frame: the streamed level's octree leaves (in the level's splat space) and the
/// collision around the car (world space).
struct LineLayer {
    pipeline: wgpu::RenderPipeline,
    surface_pipeline: Option<wgpu::RenderPipeline>,
    layout: wgpu::BindGroupLayout,
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
            .request_device(&wgpu::DeviceDescriptor { required_limits, required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY, ..Default::default() })
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
        let mut post = PostLayer::new(&device, &dof_view, &depth_car, &depth_level);
        let post_raw = post.bind(&device, &color_view, &depth_car, &depth_level);
        if let Some(presenter) = presenter.as_mut() {
            presenter.blit_group = Some(blit_group(&device, presenter, &post_view));
        }
        let pipelines = Pipelines::new(&device, COLOR_FORMAT).map_err(|e| format!("build pipelines: {e}"))?;
        let car = match bundle {
            Some(bundle) => Some(scene::build_car(&device, &queue, &pipelines, bundle, CAR_DEPTH_ALPHA).map_err(|e| format!("build car: {e}"))?),
            None => None,
        };
        let mut overlay = OverlayLayer::new(&device);
        let mut lines = LineLayer::new(&device);
        if let Some(presenter) = &presenter {
            let format = presenter.config.format.add_srgb_suffix();
            post.surface_pipeline = Some(PostLayer::pipeline_for(&device, &post.layout, format));
            overlay.surface_pipeline = Some(OverlayLayer::pipeline_for(&device, &overlay.layout, format));
            lines.surface_pipeline = Some(LineLayer::pipeline_for(&device, &lines.layout, format));
        }
        let tires = crate::tire::TireLayer::new(&device, &depth_level, &depth_car);
        let dust = crate::dust::DustLayer::new(&device, &depth_level, &depth_car);
        let gpu_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let gpu_timer = device.features().contains(wgpu::Features::TIMESTAMP_QUERY).then(||crate::timing::GpuTimer::new(&device,&queue,gpu_ms.clone()));
        Ok(Self {
            adapter_info:adapter.get_info(),
            tires,
            dust,
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
            post_raw,
            depth_level,
            depth_car,
            pipelines,
            car,
            level: Vec::new(),
            level_model: camera::IDENTITY,
            cutout: None,
            lod_focus: None,
            shadow: None,
            shadow_softness:0.12,
            car_light: None,
            sky: None,
            visibility: [true; 3],
            exposure: 1.0,
            frames_presented: 0,
            direct_present: std::cell::Cell::new(true),
            overlay,
            lines,
            clock: None,
            gpu_ms,
            queue_ms: Default::default(),
            in_flight: Default::default(),
            gpu_timer,
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

    /// Toggle scene shader work without dropping GPU resources or changing the simulation.
    pub fn set_visibility(&mut self, sky: bool, level: bool, car: bool) {
        self.visibility = [sky, level, car];
    }

    /// Casts the surface-fitted car proxy on the level using the shared world sun; attenuation is in linear light.
    /// Darkens by up to `strength`
    /// (0..1); `None` turns it off.
    pub fn set_shadow(&mut self, shadow: Option<(f32, [f32; 3])>) {
        self.shadow = shadow;
    }

    pub fn set_shadow_softness(&mut self,softness:f32){self.shadow_softness=softness.clamp(0.03,0.35);}
    pub fn set_car_light(&mut self, light: Option<crate::surface::CarLight>) {
        self.car_light = light;
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

    /// GPU execution milliseconds from timestamp queries, with queue-completion latency as a fallback.
    pub fn gpu_ms(&self) -> f64 {
        if self.gpu_timer.is_some() { f64::from_bits(self.gpu_ms.load(std::sync::atomic::Ordering::Relaxed)) }
        else { self.queue_latency_ms() }
    }

    pub fn queue_latency_ms(&self) -> f64 { f64::from_bits(self.queue_ms.load(std::sync::atomic::Ordering::Relaxed)) }
    pub fn timing_source(&self) -> &'static str { if self.gpu_timer.is_some() { "gpu-timestamps" } else { "queue-completion-latency" } }
    pub fn rendered_frames(&self)->u64 {self.frames_presented}
    pub fn pending_frames(&self) -> u32 { self.in_flight.load(std::sync::atomic::Ordering::Relaxed) }
    /// Latest asynchronous GPU pass trace and rolling aggregates. Missing timestamps remain null.
    pub fn profile_json(&self)->serde_json::Value {
        let mut value=self.gpu_timer.as_ref().map_or_else(||serde_json::json!({"passes":[],"gpu_frame_ms":null}),|t|t.snapshot());
        value["schema_version"]=1.into();value["timing_source"]=self.timing_source().into();
        value["shader_catalogue"]=crate::timing::catalogue();
        value["visibility"]=serde_json::json!({"sky":self.visibility[0],"level":self.visibility[1],"car":self.visibility[2]});
        value["tire_marks"]=self.tires.profile_json();value["dust"]=self.dust.profile_json();
        value["queue_latency_ms"]=self.queue_latency_ms().into();value["pending_frames"]=self.pending_frames().into();
        value["width"]=self.width.into();value["height"]=self.height.into();value["rendered_frames"]=self.frames_presented.into();
        value["scene_slots"]=serde_json::json!({"car":self.car.as_ref().map_or(0,|c|c.capacity()),"level":self.level.iter().map(|c|u64::from(c.capacity())).sum::<u64>()});
        value["presentation"]=serde_json::json!({"path":if self.presenter.is_none(){"offscreen"}else if self.direct_present.get(){"direct_surface"}else{"offscreen_blit"},"surface_format":self.presenter.as_ref().map(|p|format!("{:?}",p.config.format.add_srgb_suffix()))});
        value["adapter"]=serde_json::json!({"name":self.adapter_info.name,"backend":format!("{:?}",self.adapter_info.backend),"device_type":format!("{:?}",self.adapter_info.device_type)});
        value["scene_metrics"]=serde_json::json!({"car":self.car.as_ref().map(|c|c.profile_json()),"level":self.level.iter().map(|c|c.profile_json()).collect::<Vec<_>>()});
        value["shadow"]=serde_json::json!({"sun_world":self.shadow.map(|(_,sun)|sun),"strength":self.shadow.map_or(0.0,|(s,_)|s),"softness":self.shadow_softness,"proxy_segments":self.car.as_ref().and_then(|c|c.shadow_proxy.as_ref()).map_or(0,|p|p.ellipsoids.len()),"highlight_occlusion":"surface-fitted proxy","receiver_attenuation":"linear light, Gaussian incidence weighted"});
        value["effects"]=serde_json::json!({"dof":self.post.settings.dof,"car_sharpen":self.post.settings.car_sharpen,"car_clarity":self.post.settings.car_clarity,"portal_frame":self.post.settings.portal_frame,"portal_width_px":self.post.settings.portal_width_px,"haze_cutoff":self.post.settings.car_haze_cutoff,"blur":self.post.settings.blur,"blur_length":self.post.settings.blur_length,"blur_start":self.post.settings.blur_start,"fringe":self.post.settings.fringe,"grain":self.post.settings.grain,"dust":self.dust.strength(),"grade":self.post.settings.grade,"grade_profile":"cinematic-v2", "grade_contrast":self.post.settings.contrast,
            "grade_saturation":self.post.settings.saturation,"grade_vignette":self.post.settings.vignette,
            "grade_exposure_ev":self.post.settings.grade_exposure_ev,"grade_temperature":self.post.settings.grade_temperature,
            "grade_tint":self.post.settings.grade_tint,"grade_shoulder":self.post.settings.grade_shoulder,
            "grade_black_lift":self.post.settings.grade_black_lift,"car_specular":self.car_light.map(|l|l.intensity)});
        value
    }
    /// Common native/browser GPU profiler controls. Host adds CPU/stream data to `perf`.
    pub fn profile_command(&self,line:&str)->Option<Result<serde_json::Value,String>> {
        let args:Vec<_>=line.split_whitespace().collect();if args.first()!=Some(&"perf"){return None;}
        Some(match args.as_slice(){
            ["perf","present",mode] if ["direct","copy"].contains(mode)=>{self.direct_present.set(*mode=="direct");Ok(serde_json::json!({"direct_present":self.direct_present.get()}))},
            ["perf"]|["perf","trace"]=>Ok(self.profile_json()),
            ["perf","shaders"]=>Ok(crate::timing::catalogue()),
            ["perf","reset"]=>{self.reset_profile();Ok(serde_json::json!({"reset":true}))},
            ["perf","sample",n]=>match n.parse::<u64>(){Ok(n) if (1..=600).contains(&n)=>{self.profile_sample_every(n);Ok(serde_json::json!({"sample_every_frames":n,"gpu_available":self.gpu_timer.is_some()}))},_=>Err("sample interval must be 1..600 frames".into())},
            _=>Err("perf | perf trace | perf shaders | perf reset | perf sample 1..600 | perf present direct/copy | perf log on/off".into())
        })
    }
    pub fn profile_sample_every(&self,n:u64){if let Some(t)=&self.gpu_timer{t.set_interval(n);}}
    pub fn reset_profile(&self){if let Some(t)=&self.gpu_timer{t.reset();}if let Some(c)=&self.car{c.reset_profile();}for c in &self.level{c.reset_profile();}self.tires.reset_profile();self.dust.reset_profile();}
    /// Bound queue latency when the GPU or browser is slower than requestAnimationFrame.
    pub fn can_render(&self) -> bool {
        let _ = self.device.poll(wgpu::PollType::Poll);
        self.pending_frames() < 2
    }

    /// Centres the streamed level's detail on a world point (the car) instead of the camera: a chase camera swings
    /// round the car, and detail measured from the camera would follow it round, swapping leaves all the way.
    /// Leaves still go coarser out of view, by the camera's frustum.
    pub fn set_lod_focus(&mut self, focus: Option<[f32; 3]>) {
        self.lod_focus = focus;
    }

    /// Sets the screen effects (blur, fringing, grain and grade). The default shows the scene unchanged.
    pub fn set_dust(&mut self, revision:u64, particles:&std::collections::VecDeque<crate::dust::DustParticle>, time:f32, strength:f32) {
        self.dust.set(revision,particles,time,strength);
    }

    pub fn post_settings(&self)->PostSettings {self.post.settings}

    pub fn set_post(&mut self, settings: PostSettings) {
        if settings.car_haze_cutoff != self.post.settings.car_haze_cutoff {
            if let Some(car)=&self.car {car.set_haze_cutoff(&self.queue,settings.car_haze_cutoff);}
        }
        self.post.settings = settings;
    }

    /// Immutable world-space contact history shared by native and WASM rendering.
    pub fn set_tire_marks(&mut self, revision:u64, marks:&std::collections::VecDeque<crate::tire::TireMark>, time:f32, strength:f32) {
        self.tires.set(revision,marks,time,strength);
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

    pub fn configure_car_wheels(&mut self, parts: &[u32]) {
        if let Some(car) = self.car.as_mut() { car.configure_wheels(&self.device, &self.queue, parts); }
    }

    pub fn set_car_wheel_animation(&mut self, animation: &crate::wheel::WheelAnimation) {
        if let Some(car) = self.car.as_mut() { car.set_wheel_animation(&self.queue, animation); }
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
        self.tires.rebind(&self.device, &self.depth_level, &self.depth_car);
        self.dust.rebind(&self.device, &self.depth_level, &self.depth_car);
        (self.post_target, self.post_view) = post_target(&self.device, width, height);
        (_, self.dof_view) = post_target(&self.device, width, height);
        self.dof.group = Some(self.dof.bind(&self.device, &self.color_view, &self.depth_level, &self.depth_car));
        self.post.group = self.post.bind(&self.device, &self.dof_view, &self.depth_car, &self.depth_level);
        self.post_raw = self.post.bind(&self.device, &self.color_view, &self.depth_car, &self.depth_level);
        if let Some(presenter) = self.presenter.as_mut() {
            presenter.blit_group = Some(blit_group(&self.device, presenter, &self.post_view));
        }
    }

    /// Renders `view`, writing final passes directly to an available surface by default.
    pub fn render(&mut self, view: &View) {
        if !self.can_render() { return; }
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
        let target = frame.as_ref().map(|frame| frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(frame.texture.format().add_srgb_suffix()),
            ..Default::default()
        }));
        let direct = self.direct_present.get() && target.is_some();
        let timing = self.draw_to(&mut encoder, view, target.as_ref().filter(|_| direct));
        if !direct {
            if let (Some(target), Some(presenter)) = (target.as_ref(), self.presenter.as_ref()) {
                blit(presenter, &mut encoder, target,self.gpu_timer.as_ref().and_then(|t|t.render("present_blit","surface",1,u64::from(self.width)*u64::from(self.height))));
            }
        }
        if let (Some(timer),Some(slot))=(&self.gpu_timer,timing) { timer.end(&mut encoder,slot); }
        self.queue.submit(Some(encoder.finish()));
        if let (Some(timer),Some(slot))=(&self.gpu_timer,timing) { timer.read(slot); }
        let pending=self.in_flight.clone();pending.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        let (clock,latency)=(self.clock,self.queue_ms.clone());let submitted=clock.map(|c|c());
        self.queue.on_submitted_work_done(move || {
            if let (Some(clock),Some(submitted))=(clock,submitted) { latency.store((clock()-submitted).to_bits(),std::sync::atomic::Ordering::Relaxed); }
            pending.fetch_sub(1,std::sync::atomic::Ordering::Relaxed);
        });
        let _ = self.device.poll(wgpu::PollType::Poll);
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
        let timing = self.draw(&mut encoder, view);
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
        if let (Some(timer),Some(slot))=(&self.gpu_timer,timing) { timer.end(&mut encoder,slot); }
        self.queue.submit(Some(encoder.finish()));
        if let (Some(timer),Some(slot))=(&self.gpu_timer,timing) { timer.read(slot); }
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

    fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &View) -> Option<usize> {
        self.draw_to(encoder, view, None)
    }

    fn draw_to(&self, encoder: &mut wgpu::CommandEncoder, view: &View, destination: Option<&wgpu::TextureView>) -> Option<usize> {
        let final_target = destination.unwrap_or(&self.post_view);
        let surface = destination.is_some();
        let timing=self.gpu_timer.as_ref().and_then(|timer|timer.begin());
        let timer=self.gpu_timer.as_ref();
        let stamp=|id,target,draws,items|timer.and_then(|t|t.render(id,target,draws,items));
        let level = if self.visibility[1] { self.level.as_slice() } else { &[] };
        let car = self.car.as_ref().filter(|_| self.visibility[2]);
        let sky = self.sky.as_ref().filter(|_| self.visibility[0]);
        let level_items=level.iter().map(|c|u64::from(c.active_slots())).sum::<u64>();
        let car_items=car.map_or(0,|c|u64::from(c.active_slots()));
        let queue = &self.queue;
        let dof_active = self.post.settings.dof > 0.0 && self.post.settings.dof_radius_px > 0.0;
        let portal = self.cutout.as_ref().filter(|_| car.is_some() && !level.is_empty()
            && self.post.settings.portal_frame > 0.0 && self.post.settings.portal_width_px > 0.0)
            .filter(|c| c.soft > 1.0 && camera::transform_point(&view.view, c.target)[2] < -0.01);
        let mut aperture = [0.0; 16];
        if let Some((cut, inverse)) = portal.zip(camera::inverse(&view.view)) {
            let target = camera::transform_point(&view.view, cut.target);
            let depth = -target[2];
            aperture = [target[0]/depth, target[1]/depth, cut.radius/depth, depth,
                cut.soft, cut.splat_reach, cut.depth_margin, cut.floor_y,
                1.0/view.projection[0], 1.0/view.projection[5], view.projection[10], view.projection[14],
                inverse[1], inverse[5], inverse[9], inverse[13]];
        }
        let mut window = aperture;
        window[10] = 1.0 / self.width as f32;
        window[11] = 1.0 / self.height as f32;
        let window = f32_bytes(&window);
        let tire_marks = self.tires.prepare(queue, view, self.clock);
        let dust = self.dust.prepare(queue, view, self.clock);
        // Surface-fitted occluders share the sun with clearcoat; no extra GPU resource binding/pass.
        let off=vec![0u8;crate::shadow::UNIFORM_BYTES as usize];
        let shadow=self.shadow.filter(|s|s.0>0.0).and_then(|(strength,sun)|{
            let car=car?;let proxy=car.shadow_proxy.as_ref()?;let to_car=camera::inverse(&view.model)?;
            Some((proxy,to_car,strength,sun))
        });
        let sun_level=shadow.and_then(|(_,_,_,sun)|camera::inverse(&self.level_model).map(|inv|crate::shadow::direction(&inv,sun)));
        for chunk in level {
            let data=shadow.zip(sun_level).map(|((proxy,to_car,strength,_),sun)|proxy.uniform(&camera::mul(&to_car,&self.level_model),sun,strength,self.shadow_softness,false));
            chunk.set_uniform(queue,"shadow_box",data.as_deref().unwrap_or(&off));
            chunk.set_uniform(queue,"cutout_window",&window);
            // The portal clips pixels consistently; use centre culling only for the legacy unframed cutout.
            chunk.prepare_timed(queue,encoder,view,&self.level_model,self.cutout.as_ref().filter(|_| car.is_some() && portal.is_none()),timer,"level");
        }
        if let Some(car)=car {
            let data=shadow.map(|(proxy,to_car,strength,sun)|proxy.uniform(&camera::IDENTITY,crate::shadow::direction(&to_car,sun),strength,self.shadow_softness,true));
            car.set_uniform(queue,"shadow_box",data.as_deref().unwrap_or(&off));
            car.set_car_light(queue,self.car_light,view);
        }
        if let Some(car) = car {
            car.prepare_timed(queue, encoder, view, &view.model,None,timer,"car");
        }

        // 1. The sky, over a cleared colour. It tests against a cleared depth, so it covers the whole frame.
        {
            let mut pass = scene::color_pass_timed(encoder, &self.color_view, &self.depth_car, true, true, "sky",stamp("sky","world",u64::from(sky.is_some()),0));
            if let Some(sky) = sky {
                sky.draw(queue, &mut pass, &view.view, &view.projection, self.exposure);
            }
        }
        // 2. The depth prepasses. Each clears its buffer, so an object that is absent hides nothing. The level's depth
        //    only matters where the car is drawn, so its prepass is clipped to the car's rectangle on screen and skipped
        //    when the car is off screen; that saves most of a second full draw of the level.
        {
            //    Depth of field and the geometry-attached portal rim need scene depth outside the car.
            let rect = if dof_active || portal.is_some() {
                Some([0, 0, self.width, self.height])
            } else {
                crate::tire::union_rect(crate::tire::union_rect(car.and_then(|car| screen_rect(car.bounds, &view.model, view)), tire_marks.rect),dust.rect)
            };
            let mut pass = scene::depth_pass_timed(encoder, &self.depth_level, "level depth",stamp("level_depth","level",if rect.is_some(){level.iter().filter(|c|c.capacity()>0).count() as u64}else{0},if rect.is_some(){level_items}else{0}));
            if let Some([x, y, w, h]) = rect {
                pass.set_scissor_rect(x, y, w, h);
                for chunk in level {
                    chunk.draw_depth(&mut pass, &self.pipelines);
                }
            }
        }
        {
            let mut pass = scene::depth_pass_timed(encoder, &self.depth_car, "car depth",stamp("car_depth","car",u64::from(car.is_some()),car_items));
            if let Some(car) = car {
                car.draw_depth(&mut pass, &self.pipelines);
            }
        }
        // 3. The level against the car's depth. Normally one merged set; unmerged chunks go farthest first.
        if !level.is_empty() {
            let eye = camera::inverse(&view.view).map_or([0.0; 3], |m| [m[12], m[13], m[14]]);
            let distance = |chunk: &GsplatScene| {
                let c = camera::transform_point(&self.level_model, chunk.centre);
                (0..3).map(|i| (c[i] - eye[i]).powi(2)).sum::<f32>()
            };
            let mut order: Vec<&GsplatScene> = level.iter().collect();
            order.sort_by(|a, b| distance(b).total_cmp(&distance(a)));
            let mut pass = scene::color_pass_timed(encoder, &self.color_view, &self.depth_car, false, false, "level",stamp("level_color","level",level.len() as u64,level_items));
            for chunk in order {
                chunk.draw_color(&mut pass, &self.pipelines);
            }
        }
        // Persistent contact decals sample both depth textures without a writable depth attachment.
        // Color multiplication preserves the scanned road and baked/dynamic shadows.
        if tire_marks.count > 0 {
            self.tires.draw(encoder, &self.color_view, tire_marks.count,
                stamp("tire_marks", "road", 1, u64::from(tire_marks.count)));
        }
        // Ground dust is laid below the car's translucent Gaussian colour tails.
        // Core depth rejects particles behind opaque body splats; this ordering also
        // keeps the incomplete depth mask from cutting noisy holes into car panels.
        if dust.count > 0 {
            self.dust.draw(encoder,&self.color_view,dust.count,stamp("dust","world",1,u64::from(dust.count)));
        }
        // 4. The car, against the level's depth. DOF/grade then process both together.
        if let Some(car) = car {
            let mut pass = scene::color_pass_timed(encoder, &self.color_view, &self.depth_level, false, false, "car",stamp("car_color","car",1,car_items));
            car.draw_color(&mut pass, &self.pipelines);
        }
        // 5. Depth of field, from the scene into its own target.
        if dof_active {
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
                timestamp_writes: stamp("depth_of_field","screen",1,u64::from(self.width)*u64::from(self.height)),
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
            let car_region=car.and_then(|c|screen_rect(c.bounds,&view.model,view)).map_or([0.0;4],|[x,y,cw,ch]| {
                [(x as f32+cw as f32*0.5)/w,(y as f32+ch as f32*0.5)/h,cw as f32*0.5/w,ch as f32*0.5/h]
            });
            let mut uniform = f32_bytes(&[
                s.blur, s.fringe, s.grain, s.grade,
                s.time, 1.0 / w, 1.0 / h, s.grade_shoulder.clamp(0.0,1.0),
                s.contrast, s.saturation, s.vignette, s.blur_start,
                s.shadow_tint[0], s.shadow_tint[1], s.shadow_tint[2], s.blur_length,
                s.highlight_tint[0], s.highlight_tint[1], s.highlight_tint[2], 0.0,
                s.car_sharpen.clamp(0.0,1.0), s.car_clarity.clamp(0.0,0.5), s.portal_frame.clamp(0.0,1.0), s.portal_width_px.clamp(0.0,24.0),
                s.grade_exposure_ev.clamp(-3.0,3.0), s.grade_temperature.clamp(-1.0,1.0),
                s.grade_tint.clamp(-1.0,1.0), s.grade_black_lift.clamp(0.0,0.04),
                car_region[0],car_region[1],car_region[2],car_region[3],
            ]);
            uniform.extend(f32_bytes(&aperture));
            queue.write_buffer(&p.uniform, 0, &uniform);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: final_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: stamp("post","screen",1,u64::from(self.width)*u64::from(self.height)),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(p.surface_pipeline.as_ref().filter(|_| surface).unwrap_or(&p.pipeline));
            pass.set_bind_group(0, if dof_active { &p.group } else { &self.post_raw }, &[]);
            pass.draw(0..3, 0..1);
        }
        // 7. Debug lines (the level's octree leaves, the collision), over the finished picture.
        let level_clip = camera::mul(&view.projection, &camera::mul(&view.view, &self.level_model));
        let world_clip = camera::mul(&view.projection, &view.view);
        for (i,(batch, clip)) in [(&self.lines.lod, level_clip), (&self.lines.world, world_clip)].into_iter().enumerate() {
            let Some((buffer, count)) = &batch.vertices else { continue };
            self.queue.write_buffer(&batch.uniform, 0, &f32_bytes(&clip));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("debug lines"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: final_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: stamp("debug_lines",if i==0 {"lod"} else {"collision"},1,u64::from(*count)),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(self.lines.surface_pipeline.as_ref().filter(|_| surface).unwrap_or(&self.lines.pipeline));
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
                    view: final_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: stamp("hud","screen",1,u64::from(w)*u64::from(h)),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // The triangle covers the viewport; the texture is mapped to the overlay's own size, then clipped.
            let full = &self.overlay.current.as_ref().expect("checked").2;
            pass.set_viewport(full[0] as f32, full[1] as f32, full[2] as f32, full[3] as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w.max(1), h.max(1));
            pass.set_pipeline(self.overlay.surface_pipeline.as_ref().filter(|_| surface).unwrap_or(&self.overlay.pipeline));
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        timing
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
    fn pipeline_for(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(layout)],
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
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        pipeline
    }

    fn new(device: &wgpu::Device, scene: &wgpu::TextureView, car_depth: &wgpu::TextureView, level_depth: &wgpu::TextureView) -> Self {
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {sample_type: wgpu::TextureSampleType::Depth,view_dimension: wgpu::TextureViewDimension::D2,multisampled:false}, count:None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {sample_type: wgpu::TextureSampleType::Depth,view_dimension: wgpu::TextureViewDimension::D2,multisampled:false}, count:None,
                },
            ],
        });
        let pipeline = Self::pipeline_for(device, &layout, COLOR_FORMAT);
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
            size: 192,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut post = Self { pipeline, surface_pipeline: None, layout, sampler, uniform, group: Self::placeholder(device), settings: PostSettings::default() };
        post.group = post.bind(device, scene, car_depth, level_depth);
        post
    }

    /// An empty bind group to hold the slot until `bind` runs.
    fn placeholder(device: &wgpu::Device) -> wgpu::BindGroup {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("post placeholder"), entries: &[] });
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("post placeholder"), layout: &layout, entries: &[] })
    }

    fn bind(&self, device: &wgpu::Device, scene: &wgpu::TextureView, car_depth: &wgpu::TextureView, level_depth: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(scene) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(car_depth) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(level_depth) },
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
    fn pipeline_for(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/overlay.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[Some(layout)],
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
                    format,
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
        pipeline
    }

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
        let pipeline = Self::pipeline_for(device, &layout, COLOR_FORMAT);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipeline, surface_pipeline: None, layout, sampler, current: None }
    }
}

impl LineLayer {
    fn pipeline_for(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug lines"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/debug_lines.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug lines"),
            bind_group_layouts: &[Some(layout)],
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
                    format,
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
        pipeline
    }

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
        let pipeline = Self::pipeline_for(device, &layout, COLOR_FORMAT);
        let (lod, world) = (LineBatch::new(device, &layout), LineBatch::new(device, &layout));
        Self { pipeline, surface_pipeline: None, layout, lod, world }
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

fn blit(presenter: &Presenter, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, timestamp_writes:Option<wgpu::RenderPassTimestampWrites<'_>>) {
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
        timestamp_writes,
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
