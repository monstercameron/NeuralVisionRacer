//! The desktop window: driving and camera input, the fixed-step clock, and the console pump.
//!
//! Driving: W/↑ and S/↓ brake opposite motion to rest, then drive forward/reverse; Shift holds the rear handbrake;
//! A/← D/→ steer, Q/E paddles, G auto/manual,
//! T rights a flipped car where it is. B shows the streamed level's octree leaves, coloured by level of detail; C the voxel collision; H the dashboard.

use crate::renderer::Renderer;
use crate::{handle_console_line, Frontend};
use splat_engine::{Command, Effect, Engine, Level};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use std::collections::HashSet;
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

/// Largest frame step accepted after a stall, so a long pause does not run hundreds of ticks at once.
const MAX_FRAME: Duration = Duration::from_millis(250);

struct App {
    engine: Engine,
    frontend: Frontend,
    console: Option<Receiver<String>>,
    last_frame: Instant,
    accumulator: f64,
    dragging: Option<MouseButton>,
    shift: bool,
    alt: bool,
    cursor: Option<(f64, f64)>,
    last_export: Option<Instant>,
    /// P stops drawing (and the simulation) so the GPU and CPU go idle; the event loop then sleeps between checks.
    stopped: bool,
    /// Driving keys held down, and the eased steering they produce.
    held: HashSet<KeyCode>,
    steer: f32,
    /// Which display refreshes render (`render.frame_pacing`); the loop sleeps until the next one is due.
    pacer: splat_engine::pacing::FramePacer,
}

/// Milliseconds since the first call: the clock for frame pacing and GPU timing.
fn clock_ms() -> f64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// A page that shows `frame.png` and reloads it every second, so the live frame is viewable in any browser.
const LIVE_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>Splat Racer live</title>\
<body style=\"margin:0;height:100vh;background:#111;display:flex;align-items:center;justify-content:center\">\
<img id=\"f\" style=\"max-width:100vw;max-height:100vh;width:auto;height:auto;object-fit:contain\">\
<script>setInterval(()=>{document.getElementById('f').src='frame.png?'+Date.now()},1000)</script>";

pub fn run(engine: Engine, frontend: Frontend, console: Option<Receiver<String>>) {
    let dir = std::path::PathBuf::from(&engine.config.live_frame_dir);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join("index.html"), LIVE_PAGE);
    }
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        engine,
        frontend,
        console,
        last_frame: Instant::now(),
        accumulator: 0.0,
        pacer: splat_engine::pacing::FramePacer::new(splat_engine::pacing::PacingMode::Auto),
        dragging: None,
        shift: false,
        alt: false,
        cursor: None,
        last_export: None,
        stopped: false,
        held: HashSet::new(),
        steer: 0.0,
    };
    event_loop.run_app(&mut app).expect("run event loop");
}

impl App {
    /// Runs queued console lines. Returns `false` when the application should exit.
    fn drain_console(&mut self) -> bool {
        let Some(rx) = self.console.as_ref() else { return true };
        let lines: Vec<String> = rx.try_iter().collect();
        for line in lines {
            if handle_console_line(&mut self.engine, &mut self.frontend, &line) == Effect::Quit {
                return false;
            }
        }
        true
    }

    /// Writes the current frame to `live.frame_dir` as `frame.png`, so a browser can view it live.
    /// The file is written under a temporary name and renamed, so a reader never sees a partial image.
    fn export_frame(&mut self) {
        let interval = Duration::from_secs_f32(self.engine.config.live_interval_s.max(0.1));
        if self.last_export.is_some_and(|t| t.elapsed() < interval) {
            return;
        }
        self.last_export = Some(Instant::now());
        let dir = std::path::PathBuf::from(&self.engine.config.live_frame_dir);
        let Some(renderer) = self.frontend.renderer.as_mut() else { return };
        let temp = dir.join("frame.tmp.png");
        match renderer.screenshot(&self.engine, &temp) {
            Ok(_) => {
                let _ = std::fs::rename(&temp, dir.join("frame.png"));
            }
            Err(message) => self.engine.log.log(self.engine.tick, Level::Error, "renderer", format!("live frame: {message}")),
        }
    }

    /// Stops or restarts drawing. Stopped, the simulation is paused too (so it does not jump on restart), nothing is
    /// drawn, and the loop wakes only a few times a second to read the console.
    fn toggle_stopped(&mut self, event_loop: &ActiveEventLoop) {
        self.stopped = !self.stopped;
        let was_paused = self.engine.paused();
        if self.stopped && !was_paused {
            self.engine.execute(Command::Pause);
        } else if !self.stopped && was_paused {
            self.engine.execute(Command::Resume);
        }
        self.last_frame = Instant::now();
        event_loop.set_control_flow(if self.stopped { ControlFlow::Wait } else { ControlFlow::Poll });
        if let Some(window) = self.frontend.renderer.as_ref().and_then(|r| r.window.as_ref()) {
            let title = &self.engine.config.window_title;
            window.set_title(&if self.stopped { format!("{title} — stopped (P to resume)") } else { title.clone() });
            window.request_redraw();
        }
        self.engine.log.log(self.engine.tick, Level::Info, "renderer", if self.stopped { "rendering stopped" } else { "rendering resumed" });
    }

    /// Turns the held keys into the car's controls. Keys are on/off, so steering eases towards full lock and back.
    fn drive(&mut self, dt: f32) {
        let has = |codes: &[KeyCode]| codes.iter().any(|c| self.held.contains(c));
        let target = f32::from(u8::from(has(&[KeyCode::KeyD, KeyCode::ArrowRight]))) - f32::from(u8::from(has(&[KeyCode::KeyA, KeyCode::ArrowLeft])));
        let rate = if target == 0.0 { 4.0 } else { 2.5 } * dt;
        self.steer += (target - self.steer).clamp(-rate, rate);
        let pedal = |codes: &[KeyCode]| if has(codes) { 1.0 } else { 0.0 };
        self.engine.set_controls(splat_car::Controls {
            throttle: pedal(&[KeyCode::KeyW, KeyCode::ArrowUp]),
            brake: pedal(&[KeyCode::KeyS, KeyCode::ArrowDown]),
            handbrake: pedal(&[KeyCode::ShiftLeft, KeyCode::ShiftRight]),
            steer: self.steer,
            // Keep a paddle request until the engine has stepped with it.
            shift: self.engine.controls().shift,
        });
    }

    /// The fixed-step clock: runs whole ticks for the elapsed wall time.
    fn advance_clock(&mut self) {
        let now = Instant::now();
        let frame = now.duration_since(self.last_frame).min(MAX_FRAME);
        self.last_frame = now;
        self.drive(frame.as_secs_f32());
        let step = 1.0 / f64::from(self.engine.config.tick_hz.max(1.0));
        self.accumulator += frame.as_secs_f64();
        while self.accumulator >= step {
            self.engine.tick();
            self.accumulator -= step;
        }
        self.engine.set_render_alpha((self.accumulator / step) as f32);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.frontend.renderer.is_some() {
            return;
        }
        let config = &self.engine.config;
        let attributes = Window::default_attributes()
            .with_title(config.window_title.clone())
            .with_inner_size(PhysicalSize::new(config.window_width, config.window_height));
        let window = Arc::new(event_loop.create_window(attributes).expect("create window"));
        match Renderer::new(Some(window), self.frontend.bundle.as_ref(), &self.frontend.level, &self.engine, (0, 0)) {
            Ok(renderer) => {
                self.engine.log.log(self.engine.tick, Level::Info, "renderer", "window ready");
                self.frontend.renderer = Some(renderer);
            }
            Err(message) => {
                self.engine.log.log(self.engine.tick, Level::Error, "renderer", &message);
                eprintln!("splat-racer: {message}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { ref event, .. } if matches!(event.physical_key, PhysicalKey::Code(code) if is_drive_key(code)) => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                match event.state {
                    ElementState::Pressed => self.held.insert(code),
                    ElementState::Released => self.held.remove(&code),
                };
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat && matches!(event.physical_key, PhysicalKey::Code(KeyCode::KeyE | KeyCode::KeyQ | KeyCode::KeyG)) => {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyE) => self.engine.set_controls(splat_car::Controls { shift: 1, ..self.engine.controls() }),
                    PhysicalKey::Code(KeyCode::KeyQ) => self.engine.set_controls(splat_car::Controls { shift: -1, ..self.engine.controls() }),
                    _ => {
                        let auto = self.engine.gearbox_auto();
                        self.engine.execute(Command::Gearbox(!auto));
                    }
                }
            }
            WindowEvent::Focused(false) => self.held.clear(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => match event.logical_key {
                Key::Named(NamedKey::Backspace) if !event.repeat => {
                    self.held.clear();
                    self.engine.set_controls(splat_car::Controls::default());
                    self.engine.execute(Command::Reset);
                }
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                Key::Named(NamedKey::Space) => {
                    let command = if self.engine.paused() { Command::Resume } else { Command::Pause };
                    self.engine.execute(command);
                }
                Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                    self.engine.execute(Command::CameraReset);
                }
                Key::Character(ref c) if c.eq_ignore_ascii_case("p") => self.toggle_stopped(event_loop),
                // Debug: the streamed level's octree leaves, coloured by LOD.
                Key::Character(ref c) if c.eq_ignore_ascii_case("b") => {
                    let on = !self.engine.config.render_debug_lod_boxes;
                    self.engine.execute(Command::Set("render.debug_lod_boxes".into(), on.to_string()));
                }
                // The dashboard.
                Key::Character(ref c) if c.eq_ignore_ascii_case("h") => {
                    let on = !self.engine.config.hud_enabled;
                    self.engine.execute(Command::Set("hud.enabled".into(), on.to_string()));
                }
                // Debug: the voxel collision around the car.
                Key::Character(ref c) if c.eq_ignore_ascii_case("c") => {
                    let on = !self.engine.config.render_debug_colliders;
                    self.engine.execute(Command::Set("render.debug_colliders".into(), on.to_string()));
                }
                // Anti-flip: right the car where it is.
                Key::Character(ref c) if c.eq_ignore_ascii_case("t") => {
                    self.engine.execute(Command::Flip);
                }
                Key::Character(ref c) if c.eq_ignore_ascii_case("f") => {
                    let follow = !self.engine.camera_follows();
                    self.engine.execute(Command::CameraFollow(follow));
                }
                _ => {}
            },
            WindowEvent::ModifiersChanged(modifiers) => {
                self.shift = modifiers.state().shift_key();
                self.alt = modifiers.state().alt_key();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.frontend.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => self.dragging = Some(button),
                ElementState::Released if self.dragging == Some(button) => self.dragging = None,
                ElementState::Released => {}
            },
            WindowEvent::CursorMoved { position, .. } => {
                let last = self.cursor.replace((position.x, position.y));
                if let (Some(button), Some((lx, ly))) = (self.dragging, last) {
                    let dx = (position.x - lx) as f32;
                    let dy = (position.y - ly) as f32;
                    // Left rotates, right (or shift-left) pans along the ground, middle (or option-left) sets the height.
                    match button {
                        MouseButton::Middle => self.engine.raise_camera(dy),
                        MouseButton::Left if self.alt => self.engine.raise_camera(dy),
                        MouseButton::Right => self.engine.pan_camera(dx, dy),
                        MouseButton::Left if self.shift => self.engine.pan_camera(dx, dy),
                        MouseButton::Left => self.engine.orbit_camera(dx, dy),
                        _ => {}
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                };
                self.engine.zoom_camera(steps);
            }
            WindowEvent::RedrawRequested if self.stopped => {}
            WindowEvent::RedrawRequested => {
                if let Some(renderer) = self.frontend.renderer.as_mut() {
                    renderer.render(&self.engine);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if !self.drain_console() || self.engine.take_exit_request() {
            event_loop.exit();
            return;
        }
        if self.stopped {
            // Wake a few times a second for console lines; draw nothing.
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200)));
            return;
        }
        // Frame pacing: render on the refreshes the pacer picks and sleep until the next one, so every frame is on
        // screen for the same time.
        self.pacer.set_mode(splat_engine::pacing::PacingMode::parse(&self.engine.config.render_frame_pacing));
        if let Some(renderer) = self.frontend.renderer.as_mut() {
            renderer.frame.set_clock(clock_ms);
            self.pacer.record_gpu_ms(renderer.frame.gpu_ms());
            let refresh = renderer.window.as_ref().and_then(|w| w.current_monitor()).and_then(|m| m.refresh_rate_millihertz());
            self.pacer.set_refresh_hint(refresh.map(|mhz| 1_000_000.0 / f64::from(mhz)));
        }
        let now = clock_ms();
        if !self.pacer.tick(now) {
            let wait = Duration::from_secs_f64(((self.pacer.next_due_ms() - now) / 1000.0).clamp(0.0, 0.1));
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + wait));
            return;
        }
        event_loop.set_control_flow(ControlFlow::Poll);
        self.advance_clock();
        self.export_frame();
        if let Some(window) = self.frontend.renderer.as_ref().and_then(|r| r.window.as_ref()) {
            window.request_redraw();
        }
    }
}

/// Keys that are held to drive (pedals and steering), as opposed to pressed once.
fn is_drive_key(code: KeyCode) -> bool {
    matches!(code, KeyCode::KeyW | KeyCode::KeyS | KeyCode::KeyA | KeyCode::KeyD | KeyCode::ArrowUp | KeyCode::ArrowDown | KeyCode::ArrowLeft | KeyCode::ArrowRight | KeyCode::ShiftLeft | KeyCode::ShiftRight)
}
