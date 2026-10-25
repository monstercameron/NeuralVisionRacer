//! The browser build. JavaScript fetches the assets and passes their bytes in; the engine ticks from
//! `requestAnimationFrame`, and each frame is drawn into a `<canvas>` through WebGPU.
//!
//! Nothing here touches the file system or threads. The same engine code runs as on the desktop.

use splat_engine::console::{self, Reply};
use splat_engine::render::{apply_effects, car_cutout, collision_debug, hud_design, hud_telemetry, level_lod, level_model, lod_focus, stream_settings, view_for};
use splat_engine::{CarLoad, Engine, EngineConfig, Level, LevelFloor, Logger};
use splat_render::env;
use splat_render::frame::Frame;
use splat_render::sog;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

/// The running game: the engine, its frame, and the clock that turns animation frames into ticks.
#[wasm_bindgen]
pub struct Viewer {
    engine: Engine,
    frame: Frame,
    /// The level chunks received so far; the level is re-merged from all of them as each arrives (without streaming).
    level_chunks: Vec<sog::Bundle>,
    /// The level's LOD stream, when `level.stream` is on (see `start_stream`).
    stream: Option<splat_render::stream::LevelStream>,
    /// Chunks received but not yet decoded. One texture is decoded per frame, so a chunk never stalls a frame for
    /// the whole decode (the browser build has no threads).
    decoding: std::collections::VecDeque<PendingChunk>,
    /// The dashboard drawn over the frame (`hud.*`; built-in designs only in the browser).
    hud: Option<splat_hud::HudDesign>,
    last_ms: Option<f64>,
    accumulator: f64,
    /// The last frame's CPU time per phase (`frame_timings`).
    timings: [f64; 6],
    /// Which animation frames render (`should_render`, `render.frame_pacing`).
    pacer: splat_engine::pacing::FramePacer,
}

#[wasm_bindgen]
impl Viewer {
    /// Builds the viewer on the canvas with id `canvas_id`.
    ///
    /// - `config_text`: the contents of `engine.conf`.
    /// - `sog`: the car's `.sog` bytes.
    /// - `level_json`, `level_bin`: the level's `scene.voxel.json` and `scene.voxel.bin`. Pass empty arrays for no
    ///   level, which keeps the flat ground plane.
    /// - `viewer_settings`: the level's `viewer-settings.json` (its starting camera hints the spawn), or empty.
    pub async fn create(canvas_id: String, config_text: String, sog: Vec<u8>, level_json: Vec<u8>, level_bin: Vec<u8>, viewer_settings: Vec<u8>) -> Result<Viewer, JsValue> {
        std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&format!("splat-racer panic: {info}").into())));

        let config = EngineConfig::parse(&config_text).map_err(|errors| {
            JsValue::from_str(&errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
        })?;
        let level_name: Level = config.log_level.parse().unwrap_or(Level::Info);
        let log = Arc::new(Logger::open(None, level_name, config.log_memory_lines).map_err(js_error)?);

        let bundle = sog::load_bytes(&sog).map_err(|e| JsValue::from_str(&format!("car splat: {e}")))?;
        let centres = sog::centres(&bundle);
        log.log(0, Level::Info, "splat", format!("loaded car: {} splats", bundle.meta.count));

        let level = if level_json.is_empty() {
            None
        } else {
            let mut floor = LevelFloor::from_json_and_bin(&level_json, &level_bin, config.level_floor_clearance_m)
                .map_err(|e| JsValue::from_str(&format!("level: {e}")))?;
            floor.set_spawn_hint_from_viewer_settings(&viewer_settings);
            log.log(0, Level::Info, "level", format!("floor: {} cells", floor.cells.len()));
            Some(floor)
        };
        let engine = Engine::new(config, None, log, CarLoad::Loaded(centres), level);

        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(&canvas_id))
            .ok_or_else(|| JsValue::from_str(&format!("no element `{canvas_id}`")))?
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| JsValue::from_str("the element is not a canvas"))?;
        let size = (canvas.width().max(1), canvas.height().max(1));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| JsValue::from_str(&format!("canvas surface: {e}")))?;
        let mut frame = Frame::new(&instance, Some(surface), size, true, Some(&bundle))
            .await
            .map_err(|e| JsValue::from_str(&e))?;

        let cubemap = env::to_cubemap(&env::procedural(1024, 512, engine.config.hdri_procedural_sun), engine.config.hdri_face_size.max(16), engine.config.hdri_rotation_degrees);
        let sky = frame.build_sky(&cubemap);
        frame.set_sky(Some(sky), engine.config.render_exposure);
        frame.set_clock(clock_ms);

        // Loaded whether or not it is shown, so `hud.enabled` (the H key) toggles it live.
        let hud = hud_design(&engine.config).ok();
        Ok(Viewer { engine, frame, level_chunks: Vec::new(), stream: None, decoding: Default::default(), hud, last_ms: None, accumulator: 0.0, timings: [0.0; 6], pacer: splat_engine::pacing::FramePacer::new(splat_engine::pacing::PacingMode::Auto) })
    }

    /// One animation frame. `now_ms` is `requestAnimationFrame`'s timestamp. Runs whole engine ticks for the elapsed
    /// time, then draws the current state.
    pub fn frame(&mut self, now_ms: f64) {
        let started = clock_ms();
        let dt = match self.last_ms {
            Some(last) => ((now_ms - last) / 1000.0).clamp(0.0, 0.25),
            None => 0.0,
        };
        self.last_ms = Some(now_ms);
        let step = 1.0 / f64::from(self.engine.config.tick_hz.max(1.0));
        self.accumulator += dt;
        while self.accumulator >= step {
            self.engine.tick();
            self.accumulator -= step;
        }
        self.engine.set_render_alpha((self.accumulator / step) as f32);
        let ticked = clock_ms();
        let (width, height) = self.frame.size();
        let view = view_for(&self.engine, width, height);
        self.frame.set_level_model(level_model(&self.engine));
        self.frame.set_cutout(car_cutout(&self.engine));
        self.frame.set_lod_focus(lod_focus(&self.engine));
        apply_effects(&self.engine, &mut self.frame, (now_ms / 1000.0) as f32);
        let (mut decoded, mut streamed) = (clock_ms(), clock_ms());
        if let Some(stream) = self.stream.as_mut() {
            decode_step(&mut self.decoding, stream);
            decoded = clock_ms();
            self.frame.stream_level(stream, &view, (now_ms / 1000.0) as f32);
            streamed = clock_ms();
        }
        self.frame.set_lod_debug(self.stream.as_ref().filter(|_| self.engine.config.render_debug_lod_boxes));
        collision_debug(&self.engine, &mut self.frame);
        match (self.hud.as_ref().filter(|_| self.engine.config.hud_enabled), hud_telemetry(&self.engine)) {
            (Some(design), Some(telemetry)) => {
                let o = splat_hud::render_at_time(design, &telemetry, width, height, (now_ms / 1000.0) as f32);
                self.frame.set_overlay(Some((o.x, o.y, o.width, o.height, &o.rgba)));
            }
            _ => self.frame.set_overlay(None),
        }
        let drawn_prep = clock_ms();
        self.frame.render(&view);
        let done = clock_ms();
        self.timings = [ticked - started, decoded - ticked, streamed - decoded, drawn_prep - streamed, done - drawn_prep, done - started];
    }

    /// Frame pacing: call on every animation frame with its timestamp, and run `frame` only when this returns true,
    /// so frames are shown for a steady number of display refreshes (see `splat_engine::pacing`).
    pub fn should_render(&mut self, now_ms: f64) -> bool {
        self.pacer.set_mode(splat_engine::pacing::PacingMode::parse(&self.engine.config.render_frame_pacing));
        self.pacer.record_gpu_ms(self.frame.gpu_ms());
        self.pacer.tick(now_ms)
    }

    /// The pacing state: display refresh (ms), refreshes per frame, median GPU time (ms).
    pub fn pacing(&self) -> Vec<f64> {
        vec![self.pacer.refresh_ms(), f64::from(self.pacer.span()), self.pacer.gpu_ms()]
    }

    /// Milliseconds the last frame spent on: engine ticks, chunk decoding, stream selection and uploads, debug views
    /// and the HUD, encoding and submitting the render, and in all (CPU time; the GPU runs after).
    pub fn frame_timings(&self) -> Vec<f64> {
        self.timings.to_vec()
    }

    /// Replaces the car with one built from a `.car` spec's text (the page passes `vehicle.spec`'s file).
    pub fn set_car_spec(&mut self, text: &str) -> Result<(), JsValue> {
        let spec = splat_car::CarSpec::parse(text).map_err(js_error)?;
        self.engine.set_car_spec(spec);
        Ok(())
    }

    /// Switches the dashboard design: a built-in name (`porsche-992-gt3rs`, `turbo-sport`) or a `.hud` file's text.
    pub fn set_hud(&mut self, design: &str) -> Result<(), JsValue> {
        let design = splat_hud::HudDesign::by_name(design.trim()).map(Ok).unwrap_or_else(|| splat_hud::HudDesign::parse(design)).map_err(js_error)?;
        self.hud = Some(design);
        Ok(())
    }

    /// Whether `level.stream` is on, so the page streams the level instead of loading one level of detail.
    pub fn streaming(&self) -> bool {
        self.engine.config.level_stream
    }

    /// Starts streaming the level: `lod_meta` is its `lod-meta.json`, `chunk_metas` a JSON array of every chunk's
    /// `meta.json` in file order (`null` for one that failed).
    pub fn start_stream(&mut self, lod_meta: Vec<u8>, chunk_metas: Vec<u8>) -> Result<(), JsValue> {
        let mut stream = splat_render::stream::LevelStream::from_json(&lod_meta, &chunk_metas, stream_settings(&self.engine.config)).map_err(js_error)?;
        self.frame.start_level_stream(&mut stream).map_err(js_error)?;
        self.engine.log.log(self.engine.tick, Level::Info, "level", format!("streaming {} leaves, pool of {} splats", stream.lod_meta().leaves.len(), stream.settings.capacity));
        self.stream = Some(stream);
        Ok(())
    }

    /// Chunk folders the stream wants now, as JSON `[[file, "dir"], ...]`; each is then marked in flight.
    pub fn level_requests(&mut self) -> String {
        let Some(stream) = self.stream.as_mut() else { return "[]".into() };
        let items: Vec<String> = stream.requests().iter().map(|(file, dir)| format!("[{file},\"{dir}\"]")).collect();
        format!("[{}]", items.join(","))
    }

    /// A requested chunk's files, in `sog::BUNDLE_FILES` order, for streaming.
    #[allow(clippy::too_many_arguments)]
    pub fn add_stream_chunk(
        &mut self,
        file: usize,
        meta: Vec<u8>,
        means_l: Vec<u8>,
        means_u: Vec<u8>,
        quats: Vec<u8>,
        scales: Vec<u8>,
        sh0: Vec<u8>,
        sh_labels: Vec<u8>,
        sh_centroids: Vec<u8>,
    ) -> Result<(), JsValue> {
        let Some(stream) = self.stream.as_mut() else { return Err(JsValue::from_str("not streaming")) };
        match sog::parse_meta_json(&meta) {
            Ok(meta) => {
                let files = vec![means_l, means_u, quats, scales, sh0, sh_labels, sh_centroids];
                self.decoding.push_back(PendingChunk { file, meta, files, textures: Default::default() });
                Ok(())
            }
            Err(e) => {
                stream.failed(file);
                Err(JsValue::from_str(&format!("level chunk {file}: {e}")))
            }
        }
    }

    /// A requested chunk could not be fetched; the stream may ask for it again.
    pub fn level_chunk_failed(&mut self, file: usize) {
        if let Some(stream) = self.stream.as_mut() {
            stream.failed(file);
        }
    }

    /// The stream's state as JSON (resident splats, leaves per LOD, chunks in memory), or `null`.
    /// The camera eye as drawn this frame (between ticks), for checking motion is smooth.
    pub fn render_eye(&self) -> Vec<f32> {
        self.engine.render_camera().0.to_vec()
    }

    /// The latest LOD changes, for debugging churn.
    pub fn stream_switches(&self) -> String {
        self.stream.as_ref().map_or_else(|| "[]".into(), |s| s.recent_switches_json())
    }

    /// The leaves nearest the LOD centre with their wanted and resident LOD, for debugging pop-in.
    pub fn stream_near(&self, n: usize) -> String {
        self.stream.as_ref().map_or_else(|| "[]".into(), |s| s.nearest_leaves_json(n))
    }

    pub fn stream_stats(&self) -> String {
        match self.stream.as_ref().map(|s| s.stats()) {
            Some(s) => format!(
                "{{\"resident\":{},\"wanted\":{},\"leaves_per_lod\":{:?},\"empty\":{},\"pending\":{},\"chunks\":{},\"in_flight\":{},\"visible\":{},\"fading\":{},\"switches\":{},\"near_switches\":{},\"detail_met\":{:.2},\"detail_scale\":{:.2}}}",
                s.resident_splats, s.wanted_splats, s.leaves_per_lod, s.leaves_empty, s.leaves_pending, s.chunks_loaded, s.chunks_in_flight, s.leaves_visible, s.fading, s.switches, s.near_switches, s.detail_met, s.detail_scale
            ),
            None => "null".into(),
        }
    }

    /// The level's chunk folders to fetch, as a JSON array (e.g. `["2_0","2_1"]`), from its `lod-meta.json`, at the
    /// level of detail `level.lod` selects.
    pub fn level_chunk_dirs(&self, lod_meta: Vec<u8>) -> Result<String, JsValue> {
        let meta = splat_render::lod::LodMeta::parse(&lod_meta).map_err(js_error)?;
        let lod = level_lod(&self.engine.config, &meta);
        let dirs = meta.chunk_dirs(lod);
        self.engine.log.log(self.engine.tick, Level::Info, "level", format!("splats lod {lod}: {} chunks", dirs.len()));
        Ok(format!("[{}]", dirs.iter().map(|d| format!("\"{d}\"")).collect::<Vec<_>>().join(",")))
    }

    /// Adds one level chunk from its files, in `sog::BUNDLE_FILES` order: meta.json, means_l, means_u, quats,
    /// scales, sh0, shN_labels, shN_centroids, and redraws the level from every chunk so far, merged so one sort
    /// orders them all. Returns the chunk's splat count.
    #[allow(clippy::too_many_arguments)]
    pub fn add_level_chunk(
        &mut self,
        meta: Vec<u8>,
        means_l: Vec<u8>,
        means_u: Vec<u8>,
        quats: Vec<u8>,
        scales: Vec<u8>,
        sh0: Vec<u8>,
        sh_labels: Vec<u8>,
        sh_centroids: Vec<u8>,
    ) -> Result<u32, JsValue> {
        let mut files = [meta, means_l, means_u, quats, scales, sh0, sh_labels, sh_centroids].map(Some);
        let bundle = sog::load_parts(|name| {
            let index = sog::BUNDLE_FILES.iter().position(|f| *f == name).ok_or_else(|| format!("unexpected file {name}"))?;
            files[index].take().ok_or_else(|| format!("{name} read twice").into())
        })
        .map_err(|e| JsValue::from_str(&format!("level chunk: {e}")))?;
        let count = bundle.meta.count;
        self.level_chunks.push(bundle);
        if let Some(note) = self.frame.set_level(&self.level_chunks).map_err(|e| JsValue::from_str(&e))? {
            self.engine.log.log(self.engine.tick, Level::Warn, "renderer", note);
        }
        Ok(count)
    }

    /// Level chunks received so far.
    pub fn level_chunks(&self) -> usize {
        self.level_chunks.len()
    }

    /// Resizes the canvas drawing buffer to match its displayed size.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.frame.resize(width.max(1), height.max(1));
    }

    /// Mouse orbit, in CSS pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.engine.orbit_camera(dx, dy);
    }

    /// Mouse pan, in CSS pixels.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.engine.pan_camera(dx, dy);
    }

    /// Scroll zoom; positive steps zoom in.
    pub fn zoom(&mut self, steps: f32) {
        self.engine.zoom_camera(steps);
    }

    /// Driver controls: throttle and brake 0..1, steer -1 (left) .. 1 (right). They stay until changed. A pending
    /// paddle shift is kept until the engine has stepped with it.
    pub fn set_controls(&mut self, throttle: f32, brake: f32, steer: f32) {
        let shift = self.engine.controls().shift;
        self.engine.set_controls(splat_car::Controls { throttle, brake, steer, shift });
    }

    /// Paddle shift: +1 up, -1 down.
    pub fn shift(&mut self, direction: i32) {
        let command = if direction > 0 { "shift up" } else { "shift down" };
        self.command(command);
    }

    /// Middle-drag height, in CSS pixels: dragging up (negative) raises the camera.
    pub fn raise(&mut self, dy: f32) {
        self.engine.raise_camera(dy);
    }

    /// `follow` for the chase camera, `free` for the orbit camera.
    pub fn camera_mode(&self) -> String {
        if self.engine.camera_follows() { "follow" } else { "free" }.to_string()
    }

    /// Switches between the chase camera and the free camera; returns the console reply.
    pub fn toggle_follow(&mut self) -> String {
        let line = if self.engine.camera_follows() { "camera free" } else { "camera follow" };
        self.command(line)
    }

    /// Runs one console line and returns its reply as one JSON line, the same format as the desktop console.
    pub fn command(&mut self, line: &str) -> String {
        match console::parse(line) {
            Ok(None) => String::new(),
            Ok(Some(command)) => self.engine.execute(command).0.to_json_line(),
            Err(message) => Reply::err("parse", message).to_json_line(),
        }
    }

    /// The engine's status as JSON.
    pub fn status(&self) -> String {
        self.engine.status_json().to_string()
    }
}

/// A streamed chunk being decoded a texture at a time: `files` in `sog::streams()` order.
struct PendingChunk {
    file: usize,
    meta: sog::Meta,
    files: Vec<Vec<u8>>,
    textures: std::collections::BTreeMap<&'static str, sog::Texture>,
}

/// Decodes the next texture of the oldest pending chunk; a chunk with all its textures goes to the stream.
fn decode_step(decoding: &mut std::collections::VecDeque<PendingChunk>, stream: &mut splat_render::stream::LevelStream) {
    let Some(chunk) = decoding.front_mut() else { return };
    let index = chunk.textures.len();
    let (name, _) = sog::streams()[index];
    match sog::decode_stream(name, &chunk.files[index]) {
        Ok(texture) => {
            chunk.textures.insert(name, texture);
            chunk.files[index] = Vec::new();
        }
        Err(error) => {
            web_sys::console::warn_1(&format!("level chunk {}: {error}", chunk.file).into());
            stream.failed(chunk.file);
            decoding.pop_front();
            return;
        }
    }
    if chunk.textures.len() == sog::streams().len() {
        let chunk = decoding.pop_front().expect("front exists");
        stream.insert(chunk.file, sog::complete(chunk.meta, chunk.textures));
    }
}

/// Milliseconds from the page's clock (`performance.now()`).
fn clock_ms() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map_or(0.0, |p| p.now())
}

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}
