//! The desktop renderer: a window (or nothing, for headless screenshots) around the shared `Frame`.
//! The scene, camera and sky come from the engine through `splat_engine::render`, the same as in the browser.

use splat_engine::render::{
    apply_effects, car_cutout, collision_debug, hud_design, hud_telemetry, level_model, lod_focus,
    stream_settings, view_for,
};
use splat_engine::{Engine, Level};
use splat_render::env;
use splat_render::frame::Frame;
use splat_render::scene::View;
use splat_render::sog::{self, Bundle};
use splat_render::stream::{LevelStream, StreamSettings};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use winit::window::Window;

pub struct Renderer {
    /// The window this renderer presents to, if any.
    pub window: Option<Arc<Window>>,
    pub frame: Frame,
    streamer: Option<Streamer>,
    /// The dashboard, drawn over every frame (`hud.*`).
    hud: Option<splat_hud::HudRenderer>,
    wheel_rig: Option<splat_engine::wheel_visual::WheelRig>,
    started: std::time::Instant,
    metrics: splat_render::metrics::Metrics,
    last_profile_log: std::time::Instant,
    profile_logging: bool,
}

/// A streamed level: the stream, and a loader thread that reads and decodes requested chunk folders, so a frame
/// never waits on the disk.
struct Streamer {
    stream: LevelStream,
    /// The clock crossfades and the keep delay run on, and seconds it has been moved ahead (`settle`).
    clock: std::time::Instant,
    skipped: f32,
    dir: PathBuf,
    requests: Sender<(usize, PathBuf)>,
    results: Receiver<(usize, Result<Bundle, String>)>,
}

impl Streamer {
    fn new(dir: &Path, settings: StreamSettings) -> Result<Self, String> {
        let meta = splat_render::lod::LodMeta::parse(
            &std::fs::read(dir.join("lod-meta.json")).map_err(|e| format!("lod-meta.json: {e}"))?,
        )?;
        let metas: Vec<Option<Vec<u8>>> = meta
            .filenames
            .iter()
            .map(|f| std::fs::read(dir.join(f)).ok())
            .collect();
        let stream = LevelStream::new(meta, &metas, settings)?;
        let (requests, jobs) = mpsc::channel::<(usize, PathBuf)>();
        let (done, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("level chunks".into())
            .spawn(move || {
                for (file, path) in jobs {
                    if done
                        .send((file, sog::load_dir(&path).map_err(|e| e.to_string())))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            stream,
            clock: std::time::Instant::now(),
            skipped: 0.0,
            dir: dir.to_path_buf(),
            requests,
            results,
        })
    }

    /// Takes in loaded chunks, selects and uploads for `view`, and sends new requests to the loader.
    fn step(&mut self, frame: &mut Frame, view: &View) {
        let now_s = self.clock.elapsed().as_secs_f32() + self.skipped;
        while let Ok((file, result)) = self.results.try_recv() {
            match result {
                Ok(bundle) => self.stream.insert(file, bundle),
                Err(_) => self.stream.failed(file),
            }
        }
        frame.stream_level(&mut self.stream, view, now_s);
        for (file, chunk) in self.stream.requests() {
            let _ = self.requests.send((file, self.dir.join(chunk)));
        }
    }

    /// Steps until every leaf shows its wanted LOD (for a one-off screenshot), waiting on the loader as needed.
    fn settle(&mut self, frame: &mut Frame, view: &View) {
        for _ in 0..600 {
            self.step(frame, view);
            let stats = self.stream.stats();
            if stats.leaves_pending == 0 && stats.leaves_empty == 0 && stats.chunks_in_flight == 0 {
                // Move the clock past the last crossfades so the picture has them finished.
                self.skipped += self.stream.settings.fade_s + 0.1;
                self.step(frame, view);
                return;
            }
            if stats.chunks_in_flight > 0 {
                if let Ok((file, result)) = self.results.recv() {
                    match result {
                        Ok(bundle) => self.stream.insert(file, bundle),
                        Err(_) => self.stream.failed(file),
                    }
                }
            }
        }
    }
}

impl Renderer {
    /// With a window it presents to the window; without one it renders offscreen at `size`, which is how headless
    /// `screenshot` works.
    pub fn new(
        window: Option<Arc<Window>>,
        bundle: Option<&Bundle>,
        level: &[Bundle],
        engine: &Engine,
        size: (u32, u32),
    ) -> Result<Self, String> {
        let instance = match &window {
            Some(window) => wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
                Box::new(window.clone()),
            )),
            None => wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle()),
        };
        let (surface, size) = match &window {
            Some(window) => {
                let inner = window.inner_size();
                (
                    Some(
                        instance
                            .create_surface(window.clone())
                            .map_err(|e| format!("create surface: {e}"))?,
                    ),
                    (inner.width, inner.height),
                )
            }
            None => (None, size),
        };
        let mut frame = pollster::block_on(Frame::new(
            &instance,
            surface,
            size,
            engine.config.window_vsync,
            bundle,
        ))?;
        frame.set_clock(splat_render::metrics::clock_ms);
        let wheel_rig = bundle.and_then(|b| splat_engine::wheel_visual::WheelRig::fit(engine, b));
        if let Some(rig) = &wheel_rig {
            frame.configure_car_wheels(&rig.parts);
        }
        let config = &engine.config;
        let streamer = if config.level_stream && !config.level_path.is_empty() {
            match Streamer::new(Path::new(&config.level_path), stream_settings(config)) {
                Ok(mut streamer) => {
                    frame.start_level_stream(&mut streamer.stream)?;
                    engine.log.log(
                        engine.tick,
                        Level::Info,
                        "level",
                        format!(
                            "streaming {} leaves, pool of {} splats",
                            streamer.stream.lod_meta().leaves.len(),
                            streamer.stream.settings.capacity
                        ),
                    );
                    Some(streamer)
                }
                Err(error) => {
                    engine.log.log(
                        engine.tick,
                        Level::Error,
                        "level",
                        format!("streaming: {error}"),
                    );
                    None
                }
            }
        } else {
            if let Some(note) = frame.set_level(level)? {
                engine.log.log(engine.tick, Level::Warn, "renderer", note);
            }
            None
        };
        // The design is loaded whether or not the HUD is shown, so `hud.enabled` (the H key) can toggle it live.
        let hud = match hud_design(config) {
            Ok(design) => Some(splat_hud::HudRenderer::new(design)),
            Err(error) => {
                engine.log.log(engine.tick, Level::Error, "hud", error);
                None
            }
        };
        let mut renderer = Self {
            window,
            frame,
            streamer,
            hud,
            wheel_rig,
            started: std::time::Instant::now(),
            metrics: Default::default(),
            last_profile_log: std::time::Instant::now(),
            profile_logging: true,
        };
        renderer.reload_environment(engine)?;
        Ok(renderer)
    }

    /// Rebuilds the sky from `hdri.*` (an empty path means the procedural sky) and `render.background`, and re-reads
    /// the HUD design, so `reload` picks up edits to either.
    pub fn reload_environment(&mut self, engine: &Engine) -> Result<(), String> {
        let config = &engine.config;
        self.hud = hud_design(config)
            .map(splat_hud::HudRenderer::new)
            .map_err(|e| engine.log.log(engine.tick, Level::Error, "hud", e))
            .ok();
        if config.render_background == "black" {
            self.frame.set_sky(None, config.render_exposure);
            return Ok(());
        }
        let procedural = || env::procedural(1024, 512, config.hdri_procedural_sun);
        let panorama = if config.hdri_path.is_empty() {
            procedural()
        } else {
            // A missing or bad file must not stop the window: log it and fall back to the procedural sky.
            match env::load_hdr(Path::new(&config.hdri_path)) {
                Ok(panorama) => panorama,
                Err(error) => {
                    engine.log.log(
                        engine.tick,
                        Level::Error,
                        "renderer",
                        format!(
                            "hdri {}: {error}; using the procedural sky",
                            config.hdri_path
                        ),
                    );
                    procedural()
                }
            }
        };
        let cubemap = env::to_cubemap(
            &panorama,
            config.hdri_face_size.max(16),
            config.hdri_rotation_degrees,
        );
        let sky = self.frame.build_sky(&cubemap);
        self.frame.set_sky(Some(sky), config.render_exposure);
        Ok(())
    }

    /// Updates wheel poses and the dashboard for the next rendered frame.
    fn prepare_frame(&mut self, engine: &Engine) {
        if let Some(rig) = &self.wheel_rig {
            self.frame.set_car_wheel_animation(
                &if engine.config.effects_enabled && engine.config.effects_wheels {
                    rig.animation(engine.render_wheels(), engine.config.render_debug_wheels)
                } else { Default::default() },
            );
        }
        let (Some(design), Some(telemetry)) = (
            self.hud.as_mut().filter(|_| engine.config.hud_enabled),
            hud_telemetry(engine),
        ) else {
            if let Some(hud) = self.hud.as_mut() {
                hud.invalidate();
            }
            self.frame.set_overlay(None);
            return;
        };
        let (width, height) = self.frame.size();
        if let Some(o) = design.update(
            &telemetry,
            width,
            height,
            self.started.elapsed().as_secs_f32(),
        ) {
            self.frame
                .set_overlay(Some((o.x, o.y, o.width, o.height, &o.rgba)));
        }
    }

    /// The level stream's state, when streaming.
    pub fn stream_stats(&self) -> Option<splat_render::stream::StreamStats> {
        self.streamer.as_ref().map(|s| s.stream.stats())
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.frame.resize(width, height);
    }

    /// Draws one frame of the engine's current state and presents it when there is a window.
    pub fn render(&mut self, engine: &Engine) {
        let started = std::time::Instant::now();
        let (width, height) = self.frame.size();
        let view = view_for(engine, width, height);
        self.frame.set_level_model(level_model(engine));
        self.frame.set_cutout(car_cutout(engine));
        self.frame.set_lod_focus(lod_focus(engine));
        apply_effects(
            engine,
            &mut self.frame,
            self.started.elapsed().as_secs_f32(),
        );
        let setup = started.elapsed().as_secs_f64() * 1000.0;
        if let Some(streamer) = self.streamer.as_mut() {
            streamer.step(&mut self.frame, &view);
        }
        let streamed = started.elapsed().as_secs_f64() * 1000.0;
        self.frame.set_lod_debug(
            self.streamer
                .as_ref()
                .map(|s| &s.stream)
                .filter(|_| engine.config.render_debug_lod_boxes),
        );
        collision_debug(engine, &mut self.frame);
        self.prepare_frame(engine);
        let prepared = started.elapsed().as_secs_f64() * 1000.0;
        let before = self.frame.rendered_frames();
        self.frame.render(&view);
        let done = started.elapsed().as_secs_f64() * 1000.0;
        self.metrics.record("setup", setup);
        self.metrics.record("stream_upload", streamed - setup);
        self.metrics.record("hud_debug", prepared - streamed);
        self.metrics.record("encode_submit", done - prepared);
        self.metrics.record("frame_cpu_total", done);
        self.metrics.count("frame_attempts", 1);
        if self.frame.rendered_frames() == before {
            self.metrics.count("backpressure_skips", 1);
        }
        if self.profile_logging
            && Level::Info >= engine.log.min_level()
            && self.last_profile_log.elapsed().as_secs_f64() >= 1.0
        {
            engine
                .log
                .metrics(engine.tick, "perf", self.profile_json(engine));
            self.last_profile_log = std::time::Instant::now();
        }
    }

    pub fn profile_json(&self, engine: &Engine) -> serde_json::Value {
        let mut value = self.frame.profile_json();
        value["cpu"] = self.metrics.json();
        value["simulation"] = engine.profile_json();
        value["streaming"] = self
            .stream_stats()
            .map_or(serde_json::Value::Null, |s| s.json());
        value["tick"] = engine.tick.into();
        value["paused"] = engine.paused().into();
        value["profile_logging"] = self.profile_logging.into();
        value
    }
    pub fn profile_command(&mut self, engine: &mut Engine, line: &str) -> Option<String> {
        let args: Vec<_> = line.split_whitespace().collect();
        if args.first() != Some(&"perf") {
            return None;
        }
        let response = match args.as_slice() {
            ["perf"] => Ok(self.profile_json(engine)),
            ["perf", "log", mode] if ["on", "off"].contains(mode) => {
                self.profile_logging = *mode == "on";
                engine.profile_logging = self.profile_logging;
                Ok(serde_json::json!({"profile_logging":self.profile_logging}))
            }
            ["perf", "reset"] => {
                self.metrics.reset();
                engine.metrics.reset();
                self.frame.reset_profile();
                Ok(serde_json::json!({"reset":true}))
            }
            _ => self.frame.profile_command(line).unwrap(),
        };
        Some(
            match response {
                Ok(data) => splat_engine::console::Reply::ok("perf", data.to_string()),
                Err(error) => splat_engine::console::Reply::err("perf", error),
            }
            .to_json_line(),
        )
    }
    /// Renders one frame and writes it to `path` as an sRGB PNG. Returns the image size.
    pub fn screenshot(&mut self, engine: &Engine, path: &Path) -> Result<(u32, u32), String> {
        let (width, height) = self.frame.size();
        let view = view_for(engine, width, height);
        self.frame.set_level_model(level_model(engine));
        self.frame.set_cutout(car_cutout(engine));
        self.frame.set_lod_focus(lod_focus(engine));
        apply_effects(
            engine,
            &mut self.frame,
            self.started.elapsed().as_secs_f32(),
        );
        if let Some(streamer) = self.streamer.as_mut() {
            streamer.settle(&mut self.frame, &view);
        }
        self.frame.set_lod_debug(
            self.streamer
                .as_ref()
                .map(|s| &s.stream)
                .filter(|_| engine.config.render_debug_lod_boxes),
        );
        collision_debug(engine, &mut self.frame);
        self.prepare_frame(engine);
        let pixels = pollster::block_on(self.frame.read_rgba(&view));
        let image =
            image::RgbaImage::from_raw(width, height, pixels).ok_or("image size mismatch")?;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        image.save(path).map_err(|e| e.to_string())?;
        Ok((width, height))
    }
}
