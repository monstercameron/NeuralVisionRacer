//! Maps the engine's state to what the renderer draws: the view, the car's model matrix, and the sky exposure.
//! The window and the browser both call these, so they draw the same frame for the same state.

use crate::engine::Engine;
use splat_render::camera;
use splat_render::frame::body_model;
use splat_render::scene::View;

/// How fast the car goes, 0 at rest to 1 at `effects.speed_ref_kmh` and beyond: the strength of every
/// speed-sensitive effect. 0 with effects off or no car.
pub fn speed_factor(engine: &Engine) -> f32 {
    if !engine.config.effects_enabled {
        return 0.0;
    }
    let kmh = engine.physics.car_output().map_or(0.0, |o| o.speed_mps.abs() * 3.6);
    (kmh / engine.config.effects_speed_ref_kmh.max(1.0)).clamp(0.0, 1.0)
}

/// The per-frame view for a `width × height` target, from the engine's camera and car pose. The chase camera drops
/// back and widens its view with speed (de-zoom), and shakes, more the faster the car goes.
pub fn view_for(engine: &Engine, width: u32, height: u32) -> View {
    let config = &engine.config;
    let s = speed_factor(engine);
    let mut fov = config.camera_fov_degrees;
    let mut view = engine.view_matrix();
    if engine.camera_follows() {
        let (mut eye, mut target) = engine.render_camera();
        let back = normalize([eye[0] - target[0], eye[1] - target[1], eye[2] - target[2]]);
        for i in 0..3 {
            eye[i] += back[i] * config.effects_dezoom_m * s;
        }
        fov += config.effects_dezoom_fov_degrees * s;
        // Shake: two octaves of smooth noise per axis, growing with the square of speed; the look point moves less.
        let t = engine.render_seconds();
        let amplitude = config.effects_shake_m * s * s;
        for (axis, seed) in [(0usize, 11.0f32), (1, 37.0), (2, 73.0)] {
            let hz = config.effects_shake_hz;
            let n = smooth_noise(t * hz + seed) * 0.7 + smooth_noise(t * hz * 2.55 + seed * 2.0) * 0.3;
            eye[axis] += n * amplitude;
            target[axis] += n * amplitude * 0.4;
        }
        view = camera::look_at(eye, target, [0.0, 1.0, 0.0]);
    }
    let aspect = width as f32 / height.max(1) as f32;
    let fov = fov.to_radians();
    View {
        model: car_model(engine),
        view,
        projection: camera::perspective(fov, aspect, engine.config.camera_near, engine.config.camera_far),
        width,
        height,
        near: engine.config.camera_near,
        far: engine.config.camera_far,
    }
}

/// Applies `[effects]` to a frame: the screen effects (peripheral blur and fringing grow with speed; grain and
/// grade are constant) and the car's shadow. Everything off when `effects.enabled` is false.
pub fn apply_effects(engine: &Engine, frame: &mut splat_render::frame::Frame, time_s: f32) {
    let config = &engine.config;
    if !config.effects_enabled {
        frame.set_post(splat_render::frame::PostSettings::default());
        frame.set_shadow(None);
        return;
    }
    let s = speed_factor(engine);
    frame.set_post(splat_render::frame::PostSettings {
        blur: config.effects_blur * s,
        blur_start: config.effects_blur_start,
        blur_length: config.effects_blur_length,
        fringe: config.effects_fringe * s,
        grain: config.effects_grain,
        grade: config.effects_grade,
        contrast: config.effects_grade_contrast,
        saturation: config.effects_grade_saturation,
        vignette: config.effects_grade_vignette,
        shadow_tint: config.effects_grade_shadow_tint,
        highlight_tint: config.effects_grade_highlight_tint,
        time: time_s,
        dof: config.effects_dof.clamp(0.0, 1.0),
        dof_focus_m: config.effects_dof_focus_m,
        dof_range_m: config.effects_dof_range_m.max(0.1),
        dof_radius_px: config.effects_dof_radius_px.max(0.0),
    });
    frame.set_shadow((config.effects_shadow > 0.0).then(|| (config.effects_shadow.min(1.0), normalize(config.effects_sun))));
}

/// The hole cut through the level so the car is always seen (`render.car_cutout*`), or `None` when off or there
/// is no car.
pub fn car_cutout(engine: &Engine) -> Option<splat_render::sort::Cutout> {
    let config = &engine.config;
    if !config.render_car_cutout {
        return None;
    }
    let pose = engine.render_car_pose()?;
    let bottom = engine.physics.car_bottom().unwrap_or(-0.6);
    Some(splat_render::sort::Cutout {
        target: pose.position,
        radius: config.render_car_cutout_radius_m.max(0.1),
        floor_y: pose.position[1] + bottom + config.render_car_cutout_floor_m,
        depth_margin: config.render_car_cutout_depth_margin_m,
        soft: config.render_car_cutout_soft,
        splat_reach: config.render_car_cutout_splat_reach_m,
    })
}

/// The collision debug view (`render.debug_colliders`): the streamed floor and wall colliders around the car,
/// rebuilt only when tiles are built or dropped.
pub fn collision_debug(engine: &Engine, frame: &mut splat_render::frame::Frame) {
    let key = if engine.config.render_debug_colliders { engine.physics.collision_generation() } else { None };
    frame.set_world_debug(key, || {
        let (floor, wall) = (splat_render::frame::FLOOR_COLOUR, splat_render::frame::WALL_COLOUR);
        engine.physics.collision_debug(2).map_or_else(Vec::new, |(_, lines)| lines.into_iter().map(|(a, b, is_wall)| (a, b, if is_wall { wall } else { floor })).collect())
    });
}

/// Where level detail is centred (`level.lod_centre`): the car while the chase camera follows it, so the detail
/// around the car holds still as the camera swings round; `None` (the camera) for `camera` or the free camera.
pub fn lod_focus(engine: &Engine) -> Option<[f32; 3]> {
    if !engine.config.level_lod_centre.trim().eq_ignore_ascii_case("car") || !engine.camera_follows() {
        return None;
    }
    engine.render_car_pose().map(|pose| pose.position)
}

/// The dashboard design `hud.design` names: a built-in design, or a `.hud` file (desktop only).
pub fn hud_design(config: &crate::EngineConfig) -> Result<splat_hud::HudDesign, String> {
    let name = config.hud_design.trim();
    if let Some(design) = splat_hud::HudDesign::by_name(name) {
        return Ok(design);
    }
    let text = std::fs::read_to_string(name).map_err(|e| format!("hud design `{name}`: {e}"))?;
    splat_hud::HudDesign::parse(&text)
}

/// What the dashboard shows, from the car's driveline, or `None` without a car.
pub fn hud_telemetry(engine: &Engine) -> Option<splat_hud::Telemetry> {
    let car = engine.physics.car()?;
    let out = car.last();
    let controls = engine.controls();
    Some(splat_hud::Telemetry {
        speed_kmh: out.speed_mps.abs() * 3.6,
        rpm: out.rpm,
        redline_rpm: car.spec.redline_rpm,
        limiter_rpm: car.spec.limiter_rpm,
        gear: out.gear,
        throttle: controls.throttle,
        brake: controls.brake,
        boost_bar: out.boost_bar,
        water_temp_c: out.water_temp_c,
        oil_temp_c: out.oil_temp_c,
        oil_pressure_bar: out.oil_pressure_bar,
        limiter: out.limiter,
        launch: out.launch,
        wheelspin: out.wheelspin,
    })
}

/// Level streaming settings from `[level]`.
pub fn stream_settings(config: &crate::EngineConfig) -> splat_render::stream::StreamSettings {
    splat_render::stream::StreamSettings {
        lod_distance_m: config.level_lod_distance_m,
        lod_error_px: config.level_lod_error_px.max(0.0),
        // 0 sizes the pool to the level.
        capacity: if config.level_pool_splats == 0 { 0 } else { config.level_pool_splats.clamp(10_000, 16_000_000) as u32 },
        max_distance_m: config.level_max_distance_m,
        uploads_per_frame: config.level_uploads_per_frame.max(1000) as u32,
        behind_bias: config.level_behind_lod_bias,
        sh_lods: config.level_sh_lods,
        cache_chunks: config.level_cache_chunks,
        max_in_flight: config.level_chunks_in_flight.max(1),
        fade_s: config.level_lod_fade_s.max(0.0),
        keep_s: config.level_lod_keep_s.max(0.0),
        any_view_m: config.level_lod_any_view_m.max(0.0),
    }
}

/// The level of detail to draw from a level's `lod-meta.json`: `level.lod` as a number, or with `auto` the finest
/// within `level.max_splats`.
pub fn level_lod(engine_config: &crate::EngineConfig, meta: &splat_render::lod::LodMeta) -> u32 {
    let requested = engine_config.level_lod.trim().parse::<i32>().unwrap_or(-1);
    meta.pick(requested, engine_config.level_max_splats as u64)
}

/// The level's splat-to-world matrix: `level.rotate_z_degrees` about Z, which puts the y-down splats into the
/// y-up space of the level's collision voxels.
pub fn level_model(engine: &Engine) -> camera::Mat4 {
    let (s, c) = engine.config.level_rotate_z_degrees.to_radians().sin_cos();
    [c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// Splat space to world space, with the car's rigid-body pose when a car exists.
pub fn car_model(engine: &Engine) -> camera::Mat4 {
    match (engine.physics.vehicle(), engine.render_car_pose()) {
        (Some(vehicle), Some(pose)) => body_model(pose.position, pose.rotation, vehicle.centroid, engine.model),
        _ => engine.model,
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    v.map(|c| c / length)
}

/// Smooth value noise in -1..1: random values at whole numbers, eased between.
fn smooth_noise(t: f32) -> f32 {
    let hash = |i: f32| {
        let x = (i * 127.1).sin() * 43758.547;
        x - x.floor()
    };
    let (i, f) = (t.floor(), t - t.floor());
    let ease = f * f * (3.0 - 2.0 * f);
    (hash(i) + (hash(i + 1.0) - hash(i)) * ease) * 2.0 - 1.0
}
