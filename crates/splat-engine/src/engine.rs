//! The engine core: configuration, the fixed-step clock, and the driver that runs the state machines in
//! `splat-game`.
//!
//! The driver owns all I/O. Every machine is a pure step; the driver feeds it events from console commands,
//! physics samples, timers and asset results, applies its effects to physics and the clock, and logs one record
//! per transition. Messages go through one FIFO inbox, so a step's effects are applied in order.
//!
//! The core has no window and no GPU. The application (`splat-app`) owns rendering. Commands that need the
//! GPU return an [`Effect`] for the application to carry out.

use crate::camera::OrbitCamera;
use crate::config::EngineConfig;
use crate::console::{Command, Reply};
use crate::level::LevelFloor;
use crate::log::{Level, Logger};
use crate::physics::Physics;
use serde_json::{json, Value};
use splat_game::asset::{self, AssetEffect, AssetEvent, AssetStage, ColliderError, FetchError};
use splat_game::game::{self, GameEffect, GameEvent, GameState, StartRequest};
use splat_game::record::{Outcome, Record};
use splat_game::vehicle::{self, SampleSummary, VehicleEffect, VehicleEvent, VehicleNotice, VehicleState, VehicleTuning};
use splat_render::camera::{transform_point, Mat4};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

/// Work the application must do after a command, which the core cannot do itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Stop the main loop.
    Quit,
    /// Rebuild the environment (HDRI and sky) from `hdri.*` and `render.*`.
    ReloadEnvironment,
}

/// The car's splat as the engine received it at startup.
#[derive(Debug, Clone, PartialEq)]
pub enum CarLoad {
    /// `splat.path` is empty: there is no car, and the asset set is trivially ready.
    NotConfigured,
    /// Splat centres in splat space, as the scan stores them.
    Loaded(Vec<[f32; 3]>),
    /// The file could not be read or decoded. The message is for the log.
    Failed(String),
}

/// Asset id of the car body, as the machines and the console name it.
pub const CAR_ASSET: &str = "car.body";
/// How long a wrecked car waits before it respawns, in seconds.
const WRECK_RESPAWN_SECONDS: f32 = 2.0;
/// The one track there is today. Race starts name it; there is no track asset yet.
const TRACK_ID: &str = "main";

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Game(GameEvent),
    Vehicle(VehicleEvent),
    Asset(AssetEvent),
}

/// What the renderer interpolates between ticks.
#[derive(Debug, Clone, Copy)]
struct TickSnapshot {
    car: Option<crate::physics::BodyPose>,
    wheels: [crate::drive::WheelVisualState; 4],
    eye: [f32; 3],
    target: [f32; 3],
}

/// A jump farther than this in one tick (a reset, a flip, a teleport) is drawn at once, not interpolated.
const SNAP_M: f32 = 3.0;

pub struct Engine {
    pub config: EngineConfig,
    pub log: Arc<Logger>,
    pub physics: Physics,
    pub tick: u64,
    pub metrics: splat_render::metrics::Metrics,
    pub profile_logging:bool,
    profile_clock: Option<fn()->f64>,
    /// Splat space to world space: the `splat.rotate_x_degrees` rotation.
    pub model: Mat4,
    game: GameState,
    vehicle: VehicleState,
    tuning: VehicleTuning,
    car_stage: AssetStage,
    car_load: CarLoad,
    /// Splat centres in world space, used to build the vehicle collider.
    world_centres: Vec<[f32; 3]>,
    last_ground_contacts: u8,
    countdown_deadline: Option<u64>,
    wreck_deadline: Option<u64>,
    stepping: bool,
    exit_requested: bool,
    inbox: VecDeque<Msg>,
    /// The car's mechanics: the built-in 992 GT3 RS unless `set_car_spec` replaces it.
    car_spec: splat_car::CarSpec,
    /// A paddle shift was requested; clear it after the next physics step so it shifts once.
    pending_shift_clear: bool,
    /// The car pose and chase camera before the latest tick, and how far real time is past that tick (0..1), for
    /// drawing between ticks (`render_car_pose`, `render_camera`).
    before_tick: Option<TickSnapshot>,
    render_alpha: f32,
    config_path: Option<PathBuf>,
    level: Option<LevelFloor>,
    /// The scene: every object is an entity with components. See `scene`.
    scene: splat_ecs::World,
    schedule: splat_ecs::Schedule,
    core: crate::scene::Core,
    /// Where the car's centroid starts and returns to: the road when a level is loaded, else `vehicle.spawn`.
    spawn: [f32; 3],
}

impl Engine {
    /// Builds the simulation and runs startup: boot, asset load, vehicle load, then the configured start mode.
    pub fn new(config: EngineConfig, config_path: Option<PathBuf>, log: Arc<Logger>, car: CarLoad, level: Option<LevelFloor>) -> Self {
        let car_spec = car_spec_from_file(&config, &log);
        let upright = splat_render::camera::rotation_x(config.splat_rotate_x_degrees.to_radians());
        let rotated: Vec<[f32; 3]> = match &car {
            CarLoad::Loaded(centres) => centres.iter().map(|c| transform_point(&upright, *c)).collect(),
            _ => Vec::new(),
        };
        // Square the scan up: its length along z and its tyres level (see `car_fit`).
        let square = if config.vehicle_align { crate::car_fit::align(&rotated) } else { splat_render::camera::IDENTITY };
        let rotate = splat_render::camera::mul(&square, &upright);
        let rotated: Vec<[f32; 3]> = rotated.iter().map(|c| transform_point(&square, *c)).collect();
        // Scale the car to its real length; the collider is built from the same scaled centres.
        let scale = car_scale(&rotated, config.vehicle_length_m);
        let model = splat_render::camera::mul(&splat_render::camera::scale(scale), &rotate);
        let world_centres: Vec<[f32; 3]> = rotated.iter().map(|c| c.map(|v| v * scale)).collect();
        let spawn = spawn_point(&config, &world_centres, level.as_ref());
        let orbit = framing_on(&world_centres, spawn);
        let follow_mode = config.camera_mode != "orbit";
        let (scene, core) = build_scene(&config, &world_centres, spawn, level.as_ref(), orbit, follow_mode);
        let mut engine = Self {
            spawn,
            level,
            scene,
            schedule: crate::scene::schedule(),
            core,
            physics: Physics::new(&config),
            tuning: VehicleTuning::default(),
            config,
            log,
            tick: 0,
            metrics:Default::default(),
            profile_logging:true,
            profile_clock:{#[cfg(not(target_arch="wasm32"))]{Some(splat_render::metrics::clock_ms as fn()->f64)}#[cfg(target_arch="wasm32")]{None}},
            model,
            game: GameState::default(),
            vehicle: VehicleState::Unloaded,
            car_stage: AssetStage::Idle,
            car_load: car,
            world_centres,
            last_ground_contacts: 0,
            countdown_deadline: None,
            wreck_deadline: None,
            stepping: false,
            exit_requested: false,
            inbox: VecDeque::new(),
            car_spec,
            pending_shift_clear: false,
            before_tick: None,
            render_alpha: 1.0,
            config_path,
        };
        engine.rebuild_world();
        engine.send(Msg::Game(GameEvent::BootDone));
        engine.send(Msg::Vehicle(VehicleEvent::Load));
        engine.pump();
        let start = match engine.config.game_start_mode.as_str() {
            "practice" => Some(GameEvent::UiStart(StartRequest::Practice)),
            "race" => Some(GameEvent::UiStart(StartRequest::Race { track: TRACK_ID.into() })),
            _ => None,
        };
        if let Some(event) = start {
            engine.send_game(event);
        }
        engine
    }

    /// The camera rig: the one component of the camera entity.
    fn rig(&self) -> &crate::scene::CameraRig {
        self.scene.get::<crate::scene::CameraRig>(self.core.camera).expect("the camera entity always has a rig")
    }

    fn rig_mut(&mut self) -> &mut crate::scene::CameraRig {
        self.scene.get_mut::<crate::scene::CameraRig>(self.core.camera).expect("the camera entity always has a rig")
    }

    /// The world-to-view matrix from the camera rig: the orbit camera, or the chase camera when it is active.
    pub fn view_matrix(&self) -> splat_render::camera::Mat4 {
        self.rig().view()
    }

    /// The car's pose as the chase camera needs it: position, rotation and velocity.
    fn car_pose(&self) -> Option<splat_camera::CarPose> {
        crate::scene::pose_of(&self.scene, self.core.car)
    }

    /// Writes the car entity's position and velocity, for after a teleport.
    fn set_car_transform(&mut self, position: [f32; 3], velocity: [f32; 3]) {
        if let Some(t) = self.scene.get_mut::<crate::scene::Transform>(self.core.car) {
            t.position = position;
        }
        if let Some(v) = self.scene.get_mut::<crate::scene::Velocity>(self.core.car) {
            v.0 = velocity;
        }
    }

    /// Copies the physics body's pose and velocity into the car entity. Physics owns the car, so this runs each tick.
    fn sync_car_entity(&mut self) {
        if let Some(pose) = self.physics.vehicle_pose() {
            let velocity = self.physics.vehicle_velocity();
            if let Some(t) = self.scene.get_mut::<crate::scene::Transform>(self.core.car) {
                t.position = pose.position;
                t.rotation = pose.rotation;
            }
            if let Some(v) = self.scene.get_mut::<crate::scene::Velocity>(self.core.car) {
                v.0 = velocity;
            }
        }
    }

    /// Runs the scene's systems once, after the physics step.
    fn run_schedule(&mut self) {
        let dt = (1.0 / self.config.tick_hz.max(1.0)) as f32;
        self.scene.insert_resource(crate::scene::Clock { dt, tick: self.tick });
        self.schedule.run(&mut self.scene);
    }

    /// Creates the chase camera behind the car if the camera does not have one yet.
    fn ensure_follow(&mut self) {
        if self.rig().follow.is_none() {
            if let Some(pose) = self.car_pose() {
                let follow = crate::scene::new_follow(&pose, &self.config);
                self.rig_mut().follow = Some(follow);
            }
        }
    }

    /// Snaps the chase camera behind the car, with no swoop. Used after a reset or respawn.
    fn snap_follow(&mut self) {
        if let Some(pose) = self.car_pose() {
            if let Some(follow) = self.rig_mut().follow.as_mut() {
                follow.snap(&pose);
            }
        }
    }

    /// Mouse orbit, in pixels. Swings the chase camera around the car, or orbits the splat.
    pub fn orbit_camera(&mut self, dx: f32, dy: f32) {
        self.rig_mut().orbit_input(dx, dy);
    }

    /// Mouse pan, in pixels: moves the active camera along the ground.
    pub fn pan_camera(&mut self, dx: f32, dy: f32) {
        self.rig_mut().pan_input(dx, dy);
    }

    /// Middle-drag height, in pixels: negative raises the active camera, positive lowers it.
    pub fn raise_camera(&mut self, dy: f32) {
        self.rig_mut().raise_input(dy);
    }

    /// Puts the active camera back: behind the car for the chase camera, the level framing for the orbit camera.
    pub fn reset_camera(&mut self) {
        if self.rig().following() {
            if let Some(follow) = self.rig_mut().follow.as_mut() {
                follow.reset_view();
            }
            self.snap_follow();
        } else {
            let orbit = framing_on(&self.world_centres, self.spawn);
            self.rig_mut().orbit = orbit;
        }
    }

    /// Scroll zoom; positive steps zoom in.
    pub fn zoom_camera(&mut self, steps: f32) {
        self.rig_mut().zoom_input(steps);
    }

    /// The active camera's eye and look point, world space.
    pub fn camera_eye_target(&self) -> ([f32; 3], [f32; 3]) {
        self.rig().eye_and_target()
    }

    /// Simulated seconds since the start.
    pub fn sim_seconds(&self) -> f32 {
        self.tick as f32 / self.config.tick_hz.max(1.0)
    }

    /// How far real time is past the latest tick, as a share of a tick (the host's leftover accumulator ÷ the
    /// step). The fixed-step clock runs 0, 1 or several ticks per frame; drawing between ticks by this share keeps
    /// motion smooth at any frame rate instead of judder.
    pub fn set_render_alpha(&mut self, alpha: f32) {
        self.render_alpha = alpha.clamp(0.0, 1.0);
    }

    /// Simulated seconds at the moment being drawn (between ticks).
    pub fn render_seconds(&self) -> f32 {
        (self.tick as f32 - 1.0 + if self.paused() { 1.0 } else { self.render_alpha }) / self.config.tick_hz.max(1.0)
    }

    /// The car's pose for drawing: between the last two ticks by the render share.
    pub fn render_car_pose(&self) -> Option<crate::physics::BodyPose> {
        let now = self.physics.vehicle_pose()?;
        let Some(before) = self.before_tick.and_then(|b| b.car) else { return Some(now) };
        if distance(before.position, now.position) > SNAP_M {
            return Some(now);
        }
        let a = if self.paused() { 1.0 } else { self.render_alpha };
        let position = [0, 1, 2].map(|i| before.position[i] + (now.position[i] - before.position[i]) * a);
        // Normalised lerp, on the same hemisphere.
        let sign = if (0..4).map(|i| before.rotation[i] * now.rotation[i]).sum::<f32>() < 0.0 { -1.0 } else { 1.0 };
        let mut rotation = [0, 1, 2, 3].map(|i| before.rotation[i] * (1.0 - a) + now.rotation[i] * sign * a);
        let length = rotation.iter().map(|c| c * c).sum::<f32>().sqrt().max(1e-6);
        rotation = rotation.map(|c| c / length);
        Some(crate::physics::BodyPose { position, rotation })
    }

    pub fn render_wheels(&self) -> [crate::drive::WheelVisualState; 4] {
        let now = self.physics.visual_wheels();
        let Some(before) = self.before_tick else { return now };
        let a = if self.paused() { 1.0 } else { self.render_alpha };
        // Reset/teleport discontinuities use the new state immediately. Interpolate unwrapped phase: a wheel
        // can turn more than PI per tick at road speed, so shortest-arc interpolation would reverse it.
        let reset = before.car.zip(self.physics.vehicle_pose()).is_some_and(|(p,q)| distance(p.position,q.position)>SNAP_M);
        if reset { return now; }
        std::array::from_fn(|i| crate::drive::WheelVisualState {
            spin: before.wheels[i].spin + (now[i].spin-before.wheels[i].spin) * f64::from(a),
            angular_speed: before.wheels[i].angular_speed + (now[i].angular_speed-before.wheels[i].angular_speed)*a,
            steering: before.wheels[i].steering + (now[i].steering-before.wheels[i].steering)*a,
            travel: before.wheels[i].travel + (now[i].travel-before.wheels[i].travel)*a,
            grounded: now[i].grounded,
        })
    }

    /// The active camera's eye and look point for drawing: the chase camera between the last two ticks by the
    /// render share; the orbit camera (moved by input, not ticks) as it is.
    pub fn render_camera(&self) -> ([f32; 3], [f32; 3]) {
        let (eye, target) = self.camera_eye_target();
        match self.before_tick.filter(|_| self.camera_follows()) {
            Some(before) if distance(before.eye, eye) <= SNAP_M => {
                let a = if self.paused() { 1.0 } else { self.render_alpha };
                let lerp = |p: [f32; 3], q: [f32; 3]| [0, 1, 2].map(|i| p[i] + (q[i] - p[i]) * a);
                (lerp(before.eye, eye), lerp(before.target, target))
            }
            _ => (eye, target),
        }
    }

    /// Whether the chase camera is the active one.
    pub fn camera_follows(&self) -> bool {
        self.rig().following()
    }

    /// Switches between the chase camera (`true`) and the orbit camera.
    pub fn set_follow_mode(&mut self, follow: bool) {
        self.ensure_follow();
        let mode = if follow && self.rig().follow.is_some() { crate::scene::CameraMode::Follow } else { crate::scene::CameraMode::Orbit };
        self.rig_mut().mode = mode;
    }

    /// The entity for a console id like `3.0`, checked to be alive.
    fn entity_of(&self, id: &str) -> Result<splat_ecs::Entity, String> {
        let entity: splat_ecs::Entity = id.parse()?;
        if self.scene.is_alive(entity) {
            Ok(entity)
        } else {
            Err(format!("no entity {id}"))
        }
    }

    /// Whether the game is paused, so the engine does not tick. Derived from the game mode.
    pub fn paused(&self) -> bool {
        self.game.engine_paused()
    }

    /// True once a state machine asked the process to exit (`quit`, or a fatal error and `quit`).
    pub fn take_exit_request(&mut self) -> bool {
        std::mem::take(&mut self.exit_requested)
    }

    /// Advances the simulation one tick unless the game is paused.
    pub fn profile_json(&self)->Value {
        let mut value=self.metrics.json();value["tire_marks"]=self.physics.tire_marks.profile_json();value["dust"]=self.physics.dust.profile_json();value["foliage"]=self.physics.foliage_profile();value["runtime"]=json!({"tick_hz":self.config.tick_hz,"substeps":self.config.physics_substeps,"bodies":self.physics.world.bodies.len(),"colliders":self.physics.world.colliders.len(),"collision_tiles":self.physics.collision_tiles(),"road_profile":self.physics.road_profile(),"paused":self.paused(),"tick":self.tick});value
    }
    pub fn set_profile_clock(&mut self,clock:fn()->f64){self.profile_clock=Some(clock);self.physics.tire_marks.set_clock(clock);self.physics.dust.set_clock(clock);}
    pub fn tick(&mut self) {
        if self.paused() && !self.stepping {
            return;
        }
        let clock=self.profile_clock;let started=clock.map(|c|c());
        let (eye, target) = self.camera_eye_target();
        self.before_tick = Some(TickSnapshot { car: self.physics.vehicle_pose(), wheels: self.physics.visual_wheels(), eye, target });
        // Keep the level's collision tiles around the car (cheap unless it crossed into new tiles).
        let around = self.physics.vehicle_pose().map_or(self.spawn, |p| p.position);
        let (built,dropped)=self.physics.stream_collision(around);
        self.metrics.count("collision_tiles_built",built as u64);self.metrics.count("collision_tiles_dropped",dropped as u64);
        let collision_done=clock.map(|c|c());
        self.physics.dust.set_level(&self.config.level_path);
        self.physics.dust.strength = if self.config.effects_enabled {self.config.effects_dust.clamp(0.0,2.0)} else {0.0};
        self.physics.step();
        let physics_done=clock.map(|c|c());
        if std::mem::take(&mut self.pending_shift_clear) {
            let mut controls = self.physics.controls();
            controls.shift = 0;
            self.physics.set_controls(controls);
        }
        self.tick += 1;
        if let Some(sample) = self.physics.sample() {
            self.last_ground_contacts = sample.ground_contacts;
            if is_live(&self.vehicle) {
                self.send(Msg::Vehicle(VehicleEvent::Sample(SampleSummary::from(sample))));
            }
        }
        self.sync_car_entity();
        self.run_schedule();
        self.check_timers();
        self.pump();
        if let (Some(start),Some(coll),Some(physics),Some(clock))=(started,collision_done,physics_done,clock){let done=clock();self.metrics.record("collision_tiles",coll-start);self.metrics.record("physics",physics-coll);self.metrics.record("ecs_events_camera",done-physics);self.metrics.record("tick_total",done-start);self.metrics.count("ticks",1);}
        let period = self.config.tick_hz.max(1.0) as u64;
        if self.tick % period == 0 {
            if self.profile_logging && Level::Info>=self.log.min_level(){self.log.metrics(self.tick,"perf.simulation",self.profile_json());}
            if let Some(pose) = self.physics.vehicle_pose() {
                self.log.log(
                    self.tick,
                    Level::Debug,
                    "physics",
                    format!("vehicle at ({:.3}, {:.3}, {:.3}), speed {:.3} m/s", pose.position[0], pose.position[1], pose.position[2], self.physics.vehicle_speed()),
                );
            }
        }
    }

    /// Runs one console command and returns its reply and any effect the application must apply.
    pub fn execute(&mut self, command: Command) -> (Reply, Effect) {
        let (reply, effect) = self.execute_inner(command);
        let effect = if self.take_exit_request() { Effect::Quit } else { effect };
        (reply, effect)
    }

    fn execute_inner(&mut self, command: Command) -> (Reply, Effect) {
        let cmd = command.name();
        match command {
            Command::Help => (Reply::ok(cmd, json!({ "commands": HELP }).to_string()), Effect::None),
            Command::Status => (Reply::ok(cmd, self.status_json().to_string()), Effect::None),
            Command::Get(path) => match self.config.get(&path) {
                Some(value) => (Reply::ok(cmd, json!({ "path": path, "value": value }).to_string()), Effect::None),
                None => (Reply::err(cmd, format!("unknown setting `{path}`")), Effect::None),
            },
            Command::Set(path, value) => match self.config.set(&path, &value) {
                Ok(()) => {
                    // Renderer-read settings and the log level apply on the next frame.
                    // Structural settings wait for `reload`.
                    let applies = if path == "engine.log_level" {
                        self.apply_log_level();
                        "live"
                    } else if path.starts_with("effects.") || matches!(path.as_str(), "render.sky" | "render.level" | "render.car" | "render.car_cutout" | "render.debug_lod_boxes" | "render.debug_colliders" | "render.debug_wheels" | "render.frame_pacing" | "hud.enabled") {
                        "live"
                    } else {
                        "reload"
                    };
                    self.log.log(self.tick, Level::Info, "console", format!("set {path} = {value} (applies {applies})"));
                    (Reply::ok(cmd, json!({ "path": path, "value": self.config.get(&path), "applies": applies }).to_string()), Effect::None)
                }
                Err(message) => (Reply::err(cmd, message), Effect::None),
            },
            Command::Dump => (Reply::ok(cmd, json!({ "text": self.config.to_text() }).to_string()), Effect::None),
            Command::Reload => self.reload(),
            Command::Logs { count, level } => {
                let records = self.log.tail(count, level);
                let items: Vec<String> = records.iter().map(|r| r.to_json()).collect();
                (Reply::ok(cmd, format!("{{\"count\":{},\"records\":[{}]}}", items.len(), items.join(","))), Effect::None)
            }
            Command::Pause => self.game_command(cmd, GameEvent::UiPause, json!({ "paused": true })),
            Command::Resume => self.game_command(cmd, GameEvent::UiResume, json!({ "paused": false })),
            Command::Step(n) => self.game_command(cmd, GameEvent::StepRequested(n), json!({ "stepped": n })),
            Command::Reset => self.game_command(cmd, GameEvent::Reset, json!({ "spawn": self.spawn })),
            Command::Wreck => self.game_command(cmd, GameEvent::Wreck, json!({})),
            Command::Flip => match self.physics.right_vehicle(FLIP_LIFT_M) {
                Ok(position) => {
                    self.log.log(self.tick, Level::Info, "vehicle", "righted (anti-flip)");
                    (Reply::ok(cmd, json!({ "position": position }).to_string()), Effect::None)
                }
                Err(e) => (Reply::err(cmd, e), Effect::None),
            },
            Command::Drive([throttle, brake, steer]) => {
                self.set_controls(splat_car::Controls { throttle, brake, steer, ..Default::default() });
                (Reply::ok(cmd, self.car_json().to_string()), Effect::None)
            }
            Command::Shift(direction) => {
                let mut controls = self.physics.controls();
                controls.shift = direction;
                self.physics.set_controls(controls);
                // One substep is enough for the gearbox to take the request; clear it so it does not repeat.
                self.pending_shift_clear = true;
                (Reply::ok(cmd, self.car_json().to_string()), Effect::None)
            }
            Command::Gearbox(auto) => match self.physics.car_mut() {
                Some(car) => {
                    car.auto = auto;
                    (Reply::ok(cmd, self.car_json().to_string()), Effect::None)
                }
                None => (Reply::err(cmd, "no car"), Effect::None),
            },
            Command::Car => (Reply::ok(cmd, self.car_json().to_string()), Effect::None),
            Command::CameraShow => (Reply::ok(cmd, self.camera_json().to_string()), Effect::None),
            Command::CameraReset => {
                self.reset_camera();
                (Reply::ok(cmd, self.camera_json().to_string()), Effect::None)
            }
            Command::CameraRaise(dy) => {
                self.raise_camera(dy);
                (Reply::ok(cmd, self.camera_json().to_string()), Effect::None)
            }
            Command::CameraFollow(follow) => {
                if !follow && self.rig().following() {
                    // The free camera starts where the chase camera is, so the switch does not jump.
                    let (eye, target) = self.rig().eye_and_target();
                    self.rig_mut().orbit = orbit_from(eye, target);
                }
                self.set_follow_mode(follow);
                (Reply::ok(cmd, self.camera_json().to_string()), Effect::None)
            }
            Command::CameraSet([ex, ey, ez, tx, ty, tz]) => {
                let (eye, target) = ([ex, ey, ez], [tx, ty, tz]);
                if (0..3).map(|i| (eye[i] - target[i]).abs()).sum::<f32>() < 1e-6 {
                    return (Reply::err(cmd, "eye and target must differ"), Effect::None);
                }
                // A placed camera is a free camera.
                self.rig_mut().orbit = orbit_from(eye, target);
                self.set_follow_mode(false);
                (Reply::ok(cmd, self.camera_json().to_string()), Effect::None)
            }
            Command::CameraOrbit(dx, dy) => {
                self.orbit_camera(dx, dy);
                (Reply::ok(cmd, self.camera_json().to_string()), Effect::None)
            }
            Command::CameraPan(dx, dy) => {
                self.pan_camera(dx, dy);
                (Reply::ok(cmd, self.camera_json().to_string()), Effect::None)
            }
            Command::CameraZoom(steps) => {
                self.zoom_camera(steps);
                (Reply::ok(cmd, self.camera_json().to_string()), Effect::None)
            }
            Command::Hdri(choice) => {
                let path = if choice.eq_ignore_ascii_case("procedural") { String::new() } else { choice };
                self.config.hdri_path = path.clone();
                self.log.log(self.tick, Level::Info, "console", format!("hdri set to `{}` (applies on reload environment)", display_hdri(&path)));
                (Reply::ok(cmd, json!({ "hdri": display_hdri(&path) }).to_string()), Effect::ReloadEnvironment)
            }
            Command::Screenshot(_) => (Reply::err(cmd, "screenshot needs the renderer; the application handles it"), Effect::None),
            Command::Entities => {
                let items: Vec<Value> = self.scene.entities().into_iter().map(|e| crate::scene::describe(&self.scene, e)).collect();
                (Reply::ok(cmd, json!({ "entities": items }).to_string()), Effect::None)
            }
            Command::Inspect(id) => match self.entity_of(&id) {
                Ok(entity) => (Reply::ok(cmd, crate::scene::describe(&self.scene, entity).to_string()), Effect::None),
                Err(message) => (Reply::err(cmd, message), Effect::None),
            },
            Command::SpawnMarker(position) => {
                use crate::scene::{Name, Transform};
                let entity = self.scene.spawn();
                let number = self.scene.entities().len();
                let _ = self.scene.insert(entity, Name(format!("marker {number}")));
                let _ = self.scene.insert(entity, Transform { position, rotation: [0.0, 0.0, 0.0, 1.0] });
                self.log.log(self.tick, Level::Info, "scene", format!("spawned {entity}"));
                (Reply::ok(cmd, crate::scene::describe(&self.scene, entity).to_string()), Effect::None)
            }
            Command::Attach { id, behaviour, args } => match self.entity_of(&id) {
                Ok(entity) => {
                    use crate::scene::{Behaviour, Transform};
                    let parsed = match (behaviour.as_str(), args.as_slice()) {
                        ("spin", [deg_per_s]) => Ok(Behaviour::Spin { deg_per_s: *deg_per_s }),
                        ("bob", [amplitude, hz]) => {
                            let base_y = self.scene.get::<Transform>(entity).map_or(0.0, |t| t.position[1]);
                            Ok(Behaviour::Bob { amplitude: *amplitude, hz: *hz, base_y })
                        }
                        ("spin", _) => Err("usage: attach <id> spin <deg/s>".to_string()),
                        ("bob", _) => Err("usage: attach <id> bob <amp> <hz>".to_string()),
                        (other, _) => Err(format!("unknown behaviour `{other}` (spin, bob)")),
                    };
                    match parsed {
                        Ok(b) => {
                            let _ = self.scene.insert(entity, b);
                            (Reply::ok(cmd, crate::scene::describe(&self.scene, entity).to_string()), Effect::None)
                        }
                        Err(message) => (Reply::err(cmd, message), Effect::None),
                    }
                }
                Err(message) => (Reply::err(cmd, message), Effect::None),
            },
            Command::Detach(id) => match self.entity_of(&id) {
                Ok(entity) => {
                    self.scene.remove::<crate::scene::Behaviour>(entity);
                    (Reply::ok(cmd, crate::scene::describe(&self.scene, entity).to_string()), Effect::None)
                }
                Err(message) => (Reply::err(cmd, message), Effect::None),
            },
            Command::Despawn(id) => match self.entity_of(&id) {
                Ok(entity) if entity == self.core.car || entity == self.core.floor || entity == self.core.camera => {
                    (Reply::err(cmd, "the car, the level floor and the camera are part of the engine"), Effect::None)
                }
                Ok(entity) => {
                    self.scene.despawn(entity);
                    self.log.log(self.tick, Level::Info, "scene", format!("despawned {entity}"));
                    (Reply::ok(cmd, json!({ "despawned": id }).to_string()), Effect::None)
                }
                Err(message) => (Reply::err(cmd, message), Effect::None),
            },
            Command::Mode(name) => {
                let event = match name.as_str() {
                    "practice" => GameEvent::UiStart(StartRequest::Practice),
                    "race" => GameEvent::UiStart(StartRequest::Race { track: TRACK_ID.into() }),
                    "menu" => GameEvent::UiQuit,
                    other => return (Reply::err(cmd, format!("unknown mode `{other}` (practice, race, menu)")), Effect::None),
                };
                self.game_command(cmd, event, json!({ "mode": name }))
            }
            Command::Quit => {
                self.log.log(self.tick, Level::Info, "console", "quit requested");
                (Reply::ok(cmd, json!({ "quit": true }).to_string()), Effect::Quit)
            }
        }
    }

    /// Sends a game event, runs the machines, and replies with the transition. A refusal is an error reply.
    fn game_command(&mut self, cmd: &'static str, event: GameEvent, data: Value) -> (Reply, Effect) {
        let record = self.send_game(event);
        match record.as_ref().map(|r| r.outcome) {
            Some(Outcome::Rejected(reason)) => (Reply::err(cmd, format!("rejected: {reason}; {}", record_message(&record))), Effect::None),
            _ => {
                let mut data = data;
                if let Value::Object(map) = &mut data {
                    map.insert("transition".into(), json!(record_message(&record)));
                    map.insert("mode".into(), json!(game::mode_tag(&self.game.mode)));
                }
                (Reply::ok(cmd, data.to_string()), Effect::None)
            }
        }
    }

    /// Re-reads the config file (if the engine has one) and asks the game to reload. The world is rebuilt
    /// through the vehicle machine, so a refused reload (during a race or pause) leaves the physics alone.
    fn reload(&mut self) -> (Reply, Effect) {
        if let Some(path) = self.config_path.clone() {
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| {
                EngineConfig::parse(&t).map_err(|errors| errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
            }) {
                Ok(config) => self.config = config,
                Err(message) => {
                    self.log.log(self.tick, Level::Error, "config", format!("reload failed: {message}"));
                    return (Reply::err("reload", message), Effect::None);
                }
            }
        }
        self.apply_log_level();
        // The car spec file too, so `reload` picks up edits to it; the rebuilt car below uses it.
        self.car_spec = car_spec_from_file(&self.config, &self.log);
        let record = self.send_game(GameEvent::Reload);
        match record.as_ref().map(|r| r.outcome) {
            Some(Outcome::Rejected(reason)) => (Reply::err("reload", format!("rejected: {reason}; {}", record_message(&record))), Effect::None),
            _ => {
                self.log.log(self.tick, Level::Info, "config", "reloaded");
                (Reply::ok("reload", json!({ "tick": self.tick, "transition": record_message(&record) }).to_string()), Effect::ReloadEnvironment)
            }
        }
    }

    fn apply_log_level(&self) {
        if let Ok(level) = self.config.log_level.parse::<Level>() {
            self.log.set_min_level(level);
        }
    }

    /// Sends one game event, runs the machines to quiescence, and returns the event's own record.
    fn send_game(&mut self, event: GameEvent) -> Option<Record> {
        self.send(Msg::Game(event));
        let records = self.pump();
        records.into_iter().find(|r| r.machine == "game")
    }

    fn send(&mut self, msg: Msg) {
        self.inbox.push_back(msg);
    }

    /// Processes queued messages until none remain. Returns every record produced, in order.
    fn pump(&mut self) -> Vec<Record> {
        let mut records = Vec::new();
        while let Some(msg) = self.inbox.pop_front() {
            records.extend(self.process(msg));
        }
        records
    }

    fn process(&mut self, msg: Msg) -> Vec<Record> {
        match msg {
            Msg::Game(event) => {
                let from = game::mode_tag(&self.game.mode);
                let tag = event.tag();
                let t = game::step_game(&self.game, event);
                let to = game::mode_tag(&t.next.mode);
                let record = Record::new("game", from, tag, to, t.outcome);
                self.game = t.next;
                self.log_record(&record, !t.effects.is_empty());
                for effect in t.effects {
                    self.apply_game_effect(effect);
                }
                self.check_asset_mirror();
                vec![record]
            }
            Msg::Vehicle(event) => {
                let from = vehicle::state_tag(&self.vehicle);
                let tag = event.tag();
                let is_sample = matches!(event, VehicleEvent::Sample(_));
                let t = vehicle::step_vehicle(&self.vehicle, event, &self.tuning);
                let to = vehicle::state_tag(&t.next);
                let record = Record::new("vehicle", from, tag, to, t.outcome);
                self.vehicle = t.next;
                // Samples arrive every tick; only the ones that change the state are logged.
                if !(is_sample && record.outcome != Outcome::Applied) {
                    self.log_record(&record, !t.effects.is_empty());
                }
                for effect in t.effects {
                    self.apply_vehicle_effect(effect);
                }
                vec![record]
            }
            Msg::Asset(event) => {
                let from = asset::stage_tag(&self.car_stage);
                let tag = event.tag();
                let t = asset::step_asset(&self.car_stage, event, true);
                let to = asset::stage_tag(&t.next);
                let record = Record::new(format!("asset {CAR_ASSET}"), from, tag, to, t.outcome);
                self.car_stage = t.next;
                self.log_record(&record, !t.effects.is_empty());
                for effect in t.effects {
                    self.apply_asset_effect(effect);
                }
                vec![record]
            }
        }
    }

    fn log_record(&self, record: &Record, has_effects: bool) {
        let loggable = record.from != record.to || has_effects || record.outcome == Outcome::Applied || matches!(record.outcome, Outcome::Rejected(_));
        if !loggable {
            return;
        }
        let level = match record.outcome {
            Outcome::Applied => Level::Info,
            Outcome::Rejected(_) => Level::Warn,
            Outcome::Unchanged | Outcome::Ignored => Level::Debug,
        };
        let module = record.machine.split(' ').next().unwrap_or("engine");
        self.log.log(self.tick, level, module, record.message());
    }

    /// The asset machine and the game must agree on readiness (invariant I7). A mismatch is a bug, so it is logged
    /// at error level rather than hidden.
    fn check_asset_mirror(&self) {
        let car_ready = matches!(self.car_stage, AssetStage::Ready) || self.car_load == CarLoad::NotConfigured;
        let settling = matches!(self.game.mode, game::Mode::Boot | game::Mode::Loading);
        if car_ready != self.game.assets_ready && !settling {
            self.log.log(self.tick, Level::Error, "game", format!("assets_ready={} but car stage is {}", self.game.assets_ready, asset::stage_tag(&self.car_stage)));
        }
    }

    fn apply_game_effect(&mut self, effect: GameEffect) {
        match effect {
            GameEffect::RequestAssets => {
                if self.car_load == CarLoad::NotConfigured {
                    // With no car there is nothing to load; an empty asset set is ready at once.
                    self.send(Msg::Game(GameEvent::AssetsChanged { all_ready: true }));
                } else {
                    self.send(Msg::Asset(AssetEvent::Request));
                }
            }
            GameEffect::ReleaseAssets => self.send(Msg::Asset(AssetEvent::Release)),
            GameEffect::VehicleRelease => {
                let on_ground = self.last_ground_contacts > 0;
                self.send(Msg::Vehicle(VehicleEvent::Release { on_ground }));
            }
            GameEffect::VehiclePark => self.send(Msg::Vehicle(VehicleEvent::Park)),
            GameEffect::VehicleUnload => self.send(Msg::Vehicle(VehicleEvent::Unload)),
            GameEffect::VehicleRebuild => self.send(Msg::Vehicle(VehicleEvent::Rebuild)),
            GameEffect::VehicleReset => self.send(Msg::Vehicle(VehicleEvent::Reset)),
            GameEffect::VehicleWreck => self.send(Msg::Vehicle(VehicleEvent::Wreck)),
            GameEffect::VehicleRespawn => self.send(Msg::Vehicle(VehicleEvent::Respawn)),
            GameEffect::StartCountdown => {
                let ticks = (self.config.game_countdown_seconds.max(0.0) * self.config.tick_hz) as u64;
                self.countdown_deadline = Some(self.tick + ticks);
            }
            GameEffect::CancelCountdown => self.countdown_deadline = None,
            GameEffect::StartWreckTimer => {
                self.wreck_deadline = Some(self.tick + (WRECK_RESPAWN_SECONDS * self.config.tick_hz) as u64);
            }
            GameEffect::CancelWreckTimers => self.wreck_deadline = None,
            GameEffect::StepEngine(n) => {
                self.stepping = true;
                for _ in 0..n {
                    self.tick();
                }
                self.stepping = false;
                self.log.log(self.tick, Level::Info, "console", format!("stepped {n} ticks"));
            }
            GameEffect::RecordRaceTime(ms) => self.log.log(self.tick, Level::Info, "game", format!("race finished in {ms} ms")),
            GameEffect::Exit => self.exit_requested = true,
        }
    }

    fn apply_vehicle_effect(&mut self, effect: VehicleEffect) {
        match effect {
            VehicleEffect::Freeze(frozen) => {
                if self.physics.set_frozen(frozen).is_err() {
                    self.log.log(self.tick, Level::Warn, "physics", "freeze with no vehicle body");
                }
            }
            VehicleEffect::Teleport => {
                let spawn = self.spawn;
                self.set_car_transform(spawn, [0.0; 3]);
                if self.physics.reset_vehicle(spawn).is_err() {
                    self.log.log(self.tick, Level::Warn, "physics", "teleport with no vehicle body");
                }
                self.snap_follow();
            }
            VehicleEffect::SpawnBody => self.spawn_vehicle_body(),

            // Dropping the body means a fresh world: the ground comes back with it, from the current config.
            VehicleEffect::DespawnBody => self.rebuild_world(),
            VehicleEffect::Notify(VehicleNotice::CarFailed) => self.send(Msg::Game(GameEvent::AssetFailed(CAR_ASSET.into()))),
            VehicleEffect::Notify(VehicleNotice::Wrecked) => self.send(Msg::Game(GameEvent::VehicleWrecked)),
            VehicleEffect::Notify(VehicleNotice::OutOfBounds) => {
                self.log.log(self.tick, Level::Warn, "vehicle", "out of bounds: respawning at the checkpoint");
            }
        }
    }

    fn apply_asset_effect(&mut self, effect: AssetEffect) {
        match effect {
            AssetEffect::StartFetch => {
                let result = match &self.car_load {
                    CarLoad::Loaded(_) => Ok(()),
                    CarLoad::Failed(message) => {
                        self.log.log(self.tick, Level::Error, "asset", format!("{CAR_ASSET}: {message}"));
                        Err(FetchError::NotFound)
                    }
                    CarLoad::NotConfigured => Err(FetchError::NotFound),
                };
                self.send(Msg::Asset(AssetEvent::Fetched(result)));
            }
            AssetEffect::StartDecode => {
                // The application decoded the file before the engine started, so decoding succeeds here.
                self.send(Msg::Asset(AssetEvent::Decoded(Ok(()))));
            }
            AssetEffect::StartCollider => {
                let result = if self.world_centres.is_empty() { Err(ColliderError::Empty) } else { Ok(()) };
                self.send(Msg::Asset(AssetEvent::ColliderBuilt(result)));
            }
            AssetEffect::NotifyReady => {
                self.send(Msg::Game(GameEvent::AssetsChanged { all_ready: true }));
                self.send(Msg::Vehicle(VehicleEvent::CarReady));
            }
            AssetEffect::NotifyFailed(failure) => {
                self.log.log(self.tick, Level::Error, "asset", format!("{CAR_ASSET} failed: {failure:?}"));
                self.send(Msg::Game(GameEvent::AssetFailed(CAR_ASSET.into())));
                self.send(Msg::Vehicle(VehicleEvent::CarFailed));
            }
            AssetEffect::Release => {}
        }
    }

    /// Fires the wall-clock timers the game asked for, once their deadline tick has passed.
    fn check_timers(&mut self) {
        if let Some(deadline) = self.countdown_deadline {
            if self.tick >= deadline {
                self.countdown_deadline = None;
                self.send(Msg::Game(GameEvent::CountdownElapsed));
            }
        }
        if let Some(deadline) = self.wreck_deadline {
            if self.tick >= deadline {
                self.wreck_deadline = None;
                self.send(Msg::Game(GameEvent::WreckTimerElapsed));
            }
        }
    }

    /// A new physics world from the current config: the ground, and no vehicle yet.
    fn rebuild_world(&mut self) {
        self.physics = Physics::new(&self.config);
        self.physics.set_spawn_yaw(spawn_yaw(&self.config, self.level.as_ref()));
        match &self.level {
            Some(level) => {
                // The floor and walls stream in tiles around the car; the first ones go in at the spawn now.
                self.physics.set_level(level, &self.config);
                let (built, _) = self.physics.stream_collision(self.spawn);
                self.log.log(self.tick, Level::Info, "physics", format!("level collision: {built} tiles of {} m around the spawn", self.config.level_collision_tile_m));
            }
            None if self.config.physics_ground => {
                self.physics.add_ground(&self.config);
            }
            None => {}
        }
    }

    pub fn status_json(&self) -> Value {
        let vehicle = self.physics.vehicle_pose().map(|pose| {
            json!({
                "state": vehicle::state_tag(&self.vehicle),
                "ground_contacts": self.last_ground_contacts,
                "position": pose.position,
                "rotation_xyzw": pose.rotation,
                "speed_mps": self.physics.vehicle_speed(),
            })
        });
        json!({
            "tick": self.tick,
            "paused": self.paused(),
            "tick_hz": self.config.tick_hz,
            "sim_seconds": self.tick as f64 / self.config.tick_hz.max(1.0) as f64,
            "game": {
                "mode": game::mode_tag(&self.game.mode),
                "assets_ready": self.game.assets_ready,
                "countdown_ends_tick": self.countdown_deadline,
            },
            "assets": { CAR_ASSET: asset::stage_tag(&self.car_stage) },
            "vehicle": vehicle,
            "car": self.car_telemetry(),
            "wheels": self.physics.visual_wheels().map(|w| json!({"spin_rad":w.spin,"angular_speed_rad_s":w.angular_speed,"steering_rad":w.steering,"travel_m":w.travel,"grounded":w.grounded})),
            "splats": self.world_centres.len(),
            "ground": self.physics.has_ground(),
            "collision_tiles": self.physics.collision_tiles(),
            "lighting": crate::lighting::status(&self.config),
            "log_level": self.log.min_level().as_str(),
        })
    }

    /// Sets the driver's controls (pedals, steering, paddles); they stay until changed.
    pub fn set_controls(&mut self, controls: splat_car::Controls) {
        self.physics.set_controls(controls);
        if controls.shift != 0 {
            self.pending_shift_clear = true;
        }
    }

    /// The driver's current controls.
    pub fn controls(&self) -> splat_car::Controls {
        self.physics.controls()
    }

    /// Whether the gearbox shifts by itself (true without a car).
    pub fn gearbox_auto(&self) -> bool {
        self.physics.car().is_none_or(|car| car.auto)
    }

    /// The car to build at the next spawn. Takes effect on `reset` or `reload`.
    pub fn set_car_spec(&mut self, spec: splat_car::CarSpec) {
        self.car_spec = spec;
        // A car on the road is rebuilt in place with the new spec (mass, wheels, suspension, engine).
        if self.physics.vehicle().is_some() {
            let at = self.physics.vehicle_pose().map_or(self.spawn, |p| p.position);
            self.physics.remove_vehicle();
            let spawn = std::mem::replace(&mut self.spawn, at);
            self.spawn_vehicle_body();
            self.spawn = spawn;
        }
    }

    /// Builds the car's body at the spawn with the current spec.
    fn spawn_vehicle_body(&mut self) {
        match self.physics.add_voxel_vehicle(&self.world_centres, &self.config, self.spawn, Some(&self.car_spec)) {
            Ok(vehicle) => {
                let spawn = self.spawn;
                self.set_car_transform(spawn, [0.0; 3]);
                self.attach_vehicle_components(vehicle);
                self.ensure_follow();
                let message = format!("vehicle body: {} occupied voxels of {} m, {} kg", vehicle.occupied_voxels, vehicle.voxel_size, vehicle.mass_kg);
                self.log.log(self.tick, Level::Info, "physics", message);
            }
            Err(message) => self.log.log(self.tick, Level::Error, "physics", format!("vehicle body failed: {message}")),
        }
    }

    /// Live driveline telemetry, or null without a car.
    fn car_telemetry(&self) -> Value {
        let (Some(out), Some(car)) = (self.physics.car_output(), self.physics.car()) else { return Value::Null };
        json!({
            "rpm": out.rpm.round(),
            "gear": match out.gear { -1 => "R".to_string(), 0 => "N".to_string(), g => g.to_string() },
            "speed_kmh": (out.speed_mps * 3.6 * 10.0).round() / 10.0,
            "torque_nm": out.engine_torque_nm.round(),
            "power_kw": out.power_kw.round(),
            "drive_force_n": out.drive_force_n.round(),
            "axle_torque_nm": out.axle_torque_nm, "drive_ratio": out.drive_ratio,
            "throttle": out.throttle, "brake": out.brake, "handbrake": out.handbrake,
            "brake_front_n": out.brake_front_n, "brake_rear_n": out.brake_rear_n,
            "handbrake_rear_n": out.handbrake_rear_n,
            "downforce_n": out.downforce_n.round(),
            "steer_deg": (out.steer_angle.to_degrees() * 10.0).round() / 10.0,
            "shifting": out.shifting,
            "limiter": out.limiter,
            "wheelspin": out.wheelspin,
            "launch": out.launch,
            "gearbox": if car.auto { "auto" } else { "manual" },
        })
    }

    /// The car's spec summary, controls and telemetry.
    fn car_json(&self) -> Value {
        let spec = &self.car_spec;
        let (kw, rpm) = spec.peak_power();
        let controls = self.physics.controls();
        json!({
            "name": spec.name,
            "layout": format!("{:?}, {:?}", spec.layout, spec.drive),
            "engine": format!("{}: {:.0} kW ({:.0} hp) at {:.0} rpm, redline {:.0}", spec.engine_type, kw, kw * 1.341, rpm, spec.redline_rpm),
            "gearbox": format!("{} {} speeds, final drive {}", spec.gearbox_type, spec.ratios.len(), spec.final_drive),
            "mass_kg": spec.mass_kg,
            "controls": { "throttle": controls.throttle, "brake": controls.brake, "steer": controls.steer, "handbrake": controls.handbrake },
            "telemetry": self.car_telemetry(),
        })
    }

    /// The active camera: where it is and looks, and its mode-specific state.
    fn camera_json(&self) -> Value {
        let rig = self.rig();
        let (eye, target) = rig.eye_and_target();
        match (rig.following(), rig.follow.as_ref()) {
            (true, Some(follow)) => json!({
                "mode": "follow",
                "eye": eye,
                "target": target,
                "distance": follow.params.distance,
                "offset": follow.offset(),
            }),
            _ => json!({
                "mode": "orbit",
                "eye": eye,
                "target": target,
                "distance": rig.orbit.distance,
                "yaw": rig.orbit.yaw,
                "pitch": rig.orbit.pitch,
            }),
        }
    }
}

/// The scale that makes the car `length_m` long, from its splat centres (already rotated upright). The length is the
/// longer horizontal extent between the 0.5th and 99.5th percentiles, so stray splats do not count. `length_m` ≤ 0
/// keeps the splat's own units.
pub fn car_scale(centres: &[[f32; 3]], length_m: f32) -> f32 {
    if length_m <= 0.0 || centres.len() < 10 {
        return 1.0;
    }
    let extent = |axis: usize| {
        let mut values: Vec<f32> = centres.iter().map(|c| c[axis]).collect();
        values.sort_by(f32::total_cmp);
        let at = |q: f32| values[((values.len() - 1) as f32 * q) as usize];
        at(0.995) - at(0.005)
    };
    let length = extent(0).max(extent(2));
    if length > 1e-4 { length_m / length } else { 1.0 }
}

/// The car's heading at the spawn: its nose (`vehicle.forward_degrees` in its own frame) turned to face the way the
/// level's starting camera looks; straight ahead (no turn) without one.
fn spawn_yaw(config: &EngineConfig, level: Option<&LevelFloor>) -> f32 {
    if config.vehicle_auto_spawn && level.is_some() {
        if let Some((_, yaw)) = saved_level_spawn(&config.level_path) { return yaw; }
    }
    match level.and_then(|l| l.spawn_forward) {
        // A turn by θ about +y moves a ground direction's angle atan2(x, z) by θ.
        Some([x, z]) => x.atan2(z) - config.vehicle_forward_degrees.to_radians(),
        None => 0.0,
    }
}

/// The car spec from the `vehicle.spec` file, so it can be edited without rebuilding; the built-in GT3 RS when the
/// file cannot be read (the browser, which passes the file's text with `set_car_spec` instead) or is invalid.
fn car_spec_from_file(config: &EngineConfig, log: &Logger) -> splat_car::CarSpec {
    let path = config.vehicle_spec.trim();
    if path.is_empty() {
        return splat_car::CarSpec::porsche_992_gt3_rs();
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match splat_car::CarSpec::parse(&text) {
            Ok(spec) => spec,
            Err(error) => {
                log.log(0, Level::Error, "vehicle", format!("{path}: {error}; using the built-in GT3 RS"));
                splat_car::CarSpec::porsche_992_gt3_rs()
            }
        },
        Err(_) => splat_car::CarSpec::porsche_992_gt3_rs(),
    }
}

/// The orbit camera at `eye` looking at `target`.
fn orbit_from(eye: [f32; 3], target: [f32; 3]) -> OrbitCamera {
    let offset = [eye[0] - target[0], eye[1] - target[1], eye[2] - target[2]];
    let distance = (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt().max(1e-3);
    OrbitCamera {
        target,
        distance,
        yaw: offset[0].atan2(offset[2]),
        pitch: (offset[1] / distance).clamp(-1.0, 1.0).asin(),
    }
}

/// Where the car's centroid starts. On a level it sits `vehicle.spawn_clearance_m` above the road, with its lowest
/// point at the road top, so the drop is short. Without a level it is `vehicle.spawn` as written.
fn spawn_point(config: &EngineConfig, centres: &[[f32; 3]], level: Option<&LevelFloor>) -> [f32; 3] {
    let Some(level) = level.filter(|_| config.vehicle_auto_spawn) else { return config.vehicle_spawn };
    if let Some((position, _)) = saved_level_spawn(&config.level_path) { return position; }
    let Some(road) = level.road_spawn() else { return config.vehicle_spawn };
    let lowest_car_point = centres.iter().map(|c| c[1]).fold(f32::INFINITY, f32::min);
    let centroid_y = crate::voxel::centroid(centres)[1];
    let bottom_below_centroid = if lowest_car_point.is_finite() { centroid_y - lowest_car_point } else { 0.0 };
    [road[0], road[1] + bottom_below_centroid + config.vehicle_spawn_clearance_m, road[2]]
}

/// Authored per-level body-centroid poses take precedence over automatic road selection.
/// Kept with the source so startup, manual reset and native/WASM builds share the same checkpoint.
fn saved_level_spawn(path: &str) -> Option<([f32; 3], f32)> {
    let profiles: serde_json::Value = serde_json::from_str(include_str!("../../../specs/level-spawns.json")).expect("saved level spawns");
    let name = path.trim_end_matches('/').rsplit('/').next()?;
    let entry = profiles["levels"].get(name)?;
    let p = entry["position"].as_array().filter(|p| p.len() == 3)?;
    let position = [p[0].as_f64()? as f32, p[1].as_f64()? as f32, p[2].as_f64()? as f32];
    let yaw = (entry["yaw_degrees"].as_f64()? as f32).to_radians();
    (position.iter().all(|p| p.is_finite()) && yaw.is_finite()).then_some((position, yaw))
}

/// The entities the engine always has: the car, the level floor and the camera.
fn build_scene(config: &EngineConfig, centres: &[[f32; 3]], spawn: [f32; 3], level: Option<&LevelFloor>, orbit: OrbitCamera, follow: bool) -> (splat_ecs::World, crate::scene::Core) {
    use crate::scene::{CameraMode, CameraRig, Collider, Name, Renderable, Transform, Velocity};
    let _ = (config, centres);
    let identity = [0.0, 0.0, 0.0, 1.0];
    let mut world = splat_ecs::World::new();

    let floor = world.spawn();
    let floor_position = level.map_or([0.0; 3], |l| l.origin);
    world.insert(floor, Name("level floor".into())).expect("new entity");
    world.insert(floor, Transform { position: floor_position, rotation: identity }).expect("new entity");
    if let Some(level) = level {
        world.insert(floor, Collider::VoxelFloor { cells: level.cells.len() }).expect("new entity");
    }

    let car = world.spawn();
    world.insert(car, Name("car".into())).expect("new entity");
    world.insert(car, Transform { position: spawn, rotation: identity }).expect("new entity");
    world.insert(car, Velocity::default()).expect("new entity");
    world.insert(car, Renderable::CarSplat).expect("new entity");

    let camera = world.spawn();
    world.insert(camera, Name("camera".into())).expect("new entity");
    let mode = if follow { CameraMode::Follow } else { CameraMode::Orbit };
    world.insert(camera, CameraRig { orbit, follow: None, mode, target: Some(car) }).expect("new entity");

    world.insert_resource(crate::scene::Clock { dt: 0.0, tick: 0 });
    (world, crate::scene::Core { car, floor, camera })
}

impl Engine {
    /// Gives the car entity its vehicle components once the body exists.
    fn attach_vehicle_components(&mut self, vehicle: crate::physics::VehicleBody) {
        use crate::scene::{Collider, Vehicle};
        let car = self.core.car;
        let _ = self.scene.insert(car, Vehicle { centroid: vehicle.centroid, mass_kg: vehicle.mass_kg });
        let _ = self.scene.insert(car, Collider::VehicleHull { cells: vehicle.occupied_voxels });
    }
}

fn is_live(state: &VehicleState) -> bool {
    matches!(
        state,
        VehicleState::Driving { .. } | VehicleState::Airborne | VehicleState::Crashed { .. } | VehicleState::Wrecked | VehicleState::Respawning { .. }
    )
}

fn record_message(record: &Option<Record>) -> String {
    record.as_ref().map_or_else(|| "no transition".to_string(), |r| r.message())
}

/// The camera framing the car at `target`, so the car sits in the centre of the view. The distance and angle come
/// from the car's size.
fn framing_on(centres: &[[f32; 3]], target: [f32; 3]) -> OrbitCamera {
    let mut camera = framing_or_default(centres);
    camera.target = target;
    camera
}

/// The camera framing the splat, or a default when there is no splat.
fn framing_or_default(centres: &[[f32; 3]]) -> OrbitCamera {
    match bounds(centres) {
        Some((lo, hi)) => OrbitCamera::framing(lo, hi),
        None => OrbitCamera { target: [0.0, 0.6, 0.0], distance: 8.0, yaw: std::f32::consts::FRAC_PI_4, pitch: 0.3 },
    }
}

fn bounds(points: &[[f32; 3]]) -> Option<([f32; 3], [f32; 3])> {
    let first = *points.first()?;
    let mut lo = first;
    let mut hi = first;
    for p in points {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    Some((lo, hi))
}

fn display_hdri(path: &str) -> &str {
    if path.is_empty() { "procedural" } else { path }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
}

/// How far the anti-flip (`flip`) lifts the car as it rights it, so it drops back onto its wheels.
const FLIP_LIFT_M: f32 = 0.5;

const HELP: &[&str] = &[
    "help", "status", "get <section.key>", "set <section.key> <value>", "dump", "reload", "logs [count] [level]",
    "pause", "resume", "step [n]", "reset", "flip", "wreck", "camera [show|reset|set ex ey ez tx ty tz|orbit dx dy|pan dx dy|zoom steps]",
    "perf | perf trace | perf shaders | perf reset | perf sample 1..600 | perf log on/off (renderer)",
    "hdri procedural|<path>", "screenshot <path.png>", "mode practice|race|menu", "entities", "entity <id>",
    "spawn marker x y z", "attach <id> spin <deg/s>|bob <amp> <hz>", "detach <id>", "despawn <id>",
    "drive <throttle> <brake> <steer>", "shift up|down", "gearbox auto|manual", "car", "quit",
];

#[cfg(test)]
mod saved_spawn_tests {
    use super::*;

    #[test]
    fn saved_delft_pose_overrides_road_selection_only_for_that_level() {
        let mut config = EngineConfig::default();
        config.level_path = "levels/trompetstraat-delft".into();
        let floor = LevelFloor { cells: vec![], origin: [0.0; 3], resolution: 1.0, dims: [0; 3],
            walls: vec![], wall_boxes: None, wall_dims: [0; 2], block_m: 1.0,
            spawn_hint: None, spawn_forward: None };
        let expected = [-3.648744, -2.9286222, -56.761772];
        assert_eq!(spawn_point(&config, &[], Some(&floor)), expected);
        assert!((spawn_yaw(&config, Some(&floor)).to_degrees() + 166.3618).abs() < 0.001);
        assert_eq!(spawn_point(&config, &[], None), config.vehicle_spawn);
        config.vehicle_auto_spawn = false;
        assert_eq!(spawn_point(&config, &[], Some(&floor)), config.vehicle_spawn);
        config.level_path = "levels/saburi-alley-yazd".into();
        assert!(saved_level_spawn(&config.level_path).is_none());
    }
}
