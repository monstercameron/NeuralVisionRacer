//! `engine.conf`: every engine setting in one INI-style file.
//!
//! Format: `[section]` headers, `key = value` lines, `#` or `;` comments (whole-line, or after a value
//! preceded by whitespace). Values are typed by the field they set: numbers, `true`/`false`, free text,
//! and `vec3` written as `x y z` or `x,y,z`. Unknown keys are errors, so typos fail loudly.
//!
//! The same `set` path serves the file loader and the agent console (`set section.key value`), so a
//! setting changed at runtime parses exactly as it would from the file.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigError {
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A value that can be written to and read from `engine.conf` text.
pub trait ConfigValue: Sized {
    fn parse_value(text: &str) -> Result<Self, String>;
    fn format_value(&self) -> String;
}

impl ConfigValue for f32 {
    fn parse_value(text: &str) -> Result<Self, String> {
        text.parse().map_err(|_| format!("expected a number, got `{text}`"))
    }
    fn format_value(&self) -> String {
        format!("{self}")
    }
}

impl ConfigValue for u32 {
    fn parse_value(text: &str) -> Result<Self, String> {
        text.parse().map_err(|_| format!("expected a whole number, got `{text}`"))
    }
    fn format_value(&self) -> String {
        format!("{self}")
    }
}

impl ConfigValue for usize {
    fn parse_value(text: &str) -> Result<Self, String> {
        text.parse().map_err(|_| format!("expected a whole number, got `{text}`"))
    }
    fn format_value(&self) -> String {
        format!("{self}")
    }
}

impl ConfigValue for bool {
    fn parse_value(text: &str) -> Result<Self, String> {
        match text {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            _ => Err(format!("expected true or false, got `{text}`")),
        }
    }
    fn format_value(&self) -> String {
        if *self { "true" } else { "false" }.to_string()
    }
}

impl ConfigValue for String {
    fn parse_value(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        let unquoted = trimmed
            .strip_prefix('"')
            .and_then(|t| t.strip_suffix('"'))
            .unwrap_or(trimmed);
        Ok(unquoted.to_string())
    }
    fn format_value(&self) -> String {
        if self.is_empty() || self.contains(char::is_whitespace) {
            format!("\"{self}\"")
        } else {
            self.clone()
        }
    }
}

impl ConfigValue for [f32; 3] {
    fn parse_value(text: &str) -> Result<Self, String> {
        let parts: Vec<&str> = text.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).collect();
        if parts.len() != 3 {
            return Err(format!("expected three numbers `x y z`, got `{text}`"));
        }
        let mut out = [0.0; 3];
        for (slot, part) in out.iter_mut().zip(parts) {
            *slot = part.parse().map_err(|_| format!("expected a number, got `{part}`"))?;
        }
        Ok(out)
    }
    fn format_value(&self) -> String {
        format!("{} {} {}", self[0], self[1], self[2])
    }
}

/// Declares every setting once: its `section.key` path, the struct field, its type and its default.
macro_rules! engine_settings {
    ($( ($path:literal, $field:ident : $ty:ty, $default:expr) ),* $(,)?) => {
        /// Every engine setting. Fields are typed; the `section.key` paths are the file and console names.
        #[derive(Debug, Clone, PartialEq)]
        pub struct EngineConfig {
            $( pub $field: $ty, )*
        }

        impl Default for EngineConfig {
            fn default() -> Self {
                Self { $( $field: $default, )* }
            }
        }

        impl EngineConfig {
            /// Every `section.key` path, in declaration order.
            pub const PATHS: &'static [&'static str] = &[$( $path, )*];

            /// Sets one value from its text form. Errors name the setting and the rejected text.
            pub fn set(&mut self, path: &str, value: &str) -> Result<(), String> {
                match path {
                    $( $path => {
                        self.$field = <$ty as ConfigValue>::parse_value(value)
                            .map_err(|e| format!("{path}: {e}"))?;
                        Ok(())
                    } )*
                    _ => Err(format!("unknown setting `{path}`")),
                }
            }

            /// Reads one value as text, or `None` for an unknown path.
            pub fn get(&self, path: &str) -> Option<String> {
                match path {
                    $( $path => Some(<$ty as ConfigValue>::format_value(&self.$field)), )*
                    _ => None,
                }
            }
        }
    };
}

engine_settings! {
    ("render.debug_wheels", render_debug_wheels: bool, false),
    // [engine]: the fixed-step clock and logging.
    ("engine.tick_hz", tick_hz: f32, 60.0),
    ("engine.log_file", log_file: String, "logs/engine.log".to_string()),
    ("engine.log_level", log_level: String, "info".to_string()),
    ("engine.log_memory_lines", log_memory_lines: usize, 500),
    // [window]: the desktop window.
    ("window.width", window_width: u32, 1280),
    ("window.height", window_height: u32, 720),
    ("window.title", window_title: String, "Splat Racer".to_string()),
    ("window.vsync", window_vsync: bool, true),
    // [render]: how the frame is composed.
    ("render.sky", render_sky: bool, true),
    ("render.level", render_level: bool, true),
    ("render.car", render_car: bool, true),
    ("render.background", render_background: String, "hdri".to_string()),
    ("render.exposure", render_exposure: f32, 1.0),
    ("render.car_cutout", render_car_cutout: bool, true),
    ("render.debug_lod_boxes", render_debug_lod_boxes: bool, true),
    ("render.frame_pacing", render_frame_pacing: String, "auto".to_string()),
    ("render.debug_colliders", render_debug_colliders: bool, true),
    ("render.car_cutout_radius_m", render_car_cutout_radius_m: f32, 2.2),
    ("render.car_cutout_floor_m", render_car_cutout_floor_m: f32, 0.3),
    ("render.car_cutout_soft", render_car_cutout_soft: f32, 1.12),
    ("render.car_cutout_splat_reach_m", render_car_cutout_splat_reach_m: f32, 0.25),
    ("render.car_cutout_depth_margin_m", render_car_cutout_depth_margin_m: f32, 0.5),
    // [camera]: the orbit camera, or the third-person chase camera that follows the car.
    ("camera.mode", camera_mode: String, "follow".to_string()),
    ("camera.fov_degrees", camera_fov_degrees: f32, 50.0),
    ("camera.reverse_swing", camera_reverse_swing: bool, true),
    ("camera.follow_distance_m", camera_follow_distance_m: f32, 6.0),
    ("camera.follow_height_m", camera_follow_height_m: f32, 1.8),
    ("camera.follow_look_height_m", camera_follow_look_height_m: f32, 0.8),
    ("camera.follow_position_damping", camera_follow_position_damping: f32, 6.0),
    ("camera.follow_look_damping", camera_follow_look_damping: f32, 10.0),
    ("camera.follow_velocity_heading_mps", camera_follow_velocity_heading_mps: f32, 2.0),
    ("camera.follow_max_pitch_degrees", camera_follow_max_pitch_degrees: f32, 69.0),
    ("camera.near", camera_near: f32, 0.01),
    ("camera.far", camera_far: f32, 1000.0),
    // [splat]: the car or scene asset.
    ("splat.path", splat_path: String, String::new()),
    ("splat.rotate_x_degrees", splat_rotate_x_degrees: f32, 180.0),
    // [hdri]: environment lighting and sky. An empty path uses the procedural sky.
    ("hdri.path", hdri_path: String, String::new()),
    ("hdri.face_size", hdri_face_size: usize, 512),
    ("hdri.rotation_degrees", hdri_rotation_degrees: f32, 0.0),
    ("hdri.procedural_sun", hdri_procedural_sun: [f32; 3], [0.4, 0.7, 0.5]),
    // [physics]: rapier3d world and voxel colliders.
    ("physics.gravity", physics_gravity: [f32; 3], [0.0, -9.81, 0.0]),
    ("physics.substeps", physics_substeps: u32, 2),
    ("physics.voxel_size", physics_voxel_size: f32, 0.05),
    ("physics.ground", physics_ground: bool, true),
    ("physics.ground_y", physics_ground_y: f32, 0.0),
    ("physics.ground_half_extent", physics_ground_half_extent: f32, 100.0),
    ("physics.friction", physics_friction: f32, 0.8),
    ("physics.restitution", physics_restitution: f32, 0.1),
    ("physics.ccd", physics_ccd: bool, true),
    ("physics.foliage_drag_per_m", foliage_drag_per_m: f32, 0.002),
    ("physics.foliage_max_decel_mps2", foliage_max_decel_mps2: f32, 0.8),
    // [level]: a streamed level whose collision voxels form the floor. An empty path keeps the ground plane.
    ("level.path", level_path: String, String::new()),
    ("level.floor_clearance_m", level_floor_clearance_m: f32, 2.0),
    ("level.stream", level_stream: bool, true),
    ("level.lod_distance_m", level_lod_distance_m: f32, 24.0),
    ("level.lod_error_px", level_lod_error_px: f32, 1.5),
    ("level.pool_splats", level_pool_splats: usize, 0),
    ("level.max_distance_m", level_max_distance_m: f32, 150.0),
    ("level.uploads_per_frame", level_uploads_per_frame: usize, 200_000),
    ("level.behind_lod_bias", level_behind_lod_bias: usize, 1),
    ("level.sh_lods", level_sh_lods: usize, 1),
    ("level.cache_chunks", level_cache_chunks: usize, 8),
    ("level.chunks_in_flight", level_chunks_in_flight: usize, 3),
    ("level.lod_fade_s", level_lod_fade_s: f32, 0.35),
    ("level.lod_keep_s", level_lod_keep_s: f32, 1.5),
    ("level.lod_centre", level_lod_centre: String, "car".to_string()),
    ("level.lod_any_view_m", level_lod_any_view_m: f32, 8.0),
    ("level.collision_tile_m", level_collision_tile_m: f32, 16.0),
    ("level.collision_radius_m", level_collision_radius_m: f32, 32.0),
    ("level.floor_smooth_m", level_floor_smooth_m: f32, 1.0),
    ("level.floor_step_keep_m", level_floor_step_keep_m: f32, 0.12),
    ("level.floor_mesh_m", level_floor_mesh_m: f32, 0.4),
    ("level.floor_max_slope", level_floor_max_slope: f32, 1.0),
    ("level.lod", level_lod: String, "auto".to_string()),
    ("level.max_splats", level_max_splats: usize, 3_000_000),
    ("level.rotate_z_degrees", level_rotate_z_degrees: f32, 180.0),
    ("level.foliage", level_foliage: bool, true),
    ("level.walls", level_walls: bool, true),
    ("level.wall_height_m", level_wall_height_m: f32, 3.0),
    // [vehicle]: the splat car as a rigid body.
    ("vehicle.mass_kg", vehicle_mass_kg: f32, 0.0),
    ("vehicle.length_m", vehicle_length_m: f32, 4.57),
    ("vehicle.forward_degrees", vehicle_forward_degrees: f32, 180.0),
    ("vehicle.align", vehicle_align: bool, true),
    ("vehicle.spec", vehicle_spec: String, "crates/splat-car/specs/porsche-992-gt3rs.car".to_string()),
    ("vehicle.hull_clearance_m", vehicle_hull_clearance_m: f32, 0.25),
    ("vehicle.hull_friction", vehicle_hull_friction: f32, 0.02),
    ("vehicle.wall_slide", vehicle_wall_slide: f32, 0.6),
    ("vehicle.suspension_sag_m", vehicle_suspension_sag_m: f32, 0.07),
    ("vehicle.suspension_rest_m", vehicle_suspension_rest_m: f32, 0.25),
    ("vehicle.damping_compression", vehicle_damping_compression: f32, 0.5),
    ("vehicle.damping_rebound", vehicle_damping_rebound: f32, 0.8),
    ("vehicle.anti_squat", vehicle_anti_squat: f32, 0.5),
    ("vehicle.anti_dive", vehicle_anti_dive: f32, 0.5),
    ("vehicle.spawn", vehicle_spawn: [f32; 3], [0.0, 3.0, 0.0]),
    ("vehicle.auto_spawn", vehicle_auto_spawn: bool, true),
    ("vehicle.spawn_clearance_m", vehicle_spawn_clearance_m: f32, 0.3),
    // [effects]: speed-sensitive camera and screen effects, and the colour grade.
    ("effects.enabled", effects_enabled: bool, true),
    ("effects.wheels", effects_wheels: bool, true),
    ("effects.speed_ref_kmh", effects_speed_ref_kmh: f32, 250.0),
    ("effects.shake_m", effects_shake_m: f32, 0.035),
    ("effects.dezoom_fov_degrees", effects_dezoom_fov_degrees: f32, 12.0),
    ("effects.dezoom_m", effects_dezoom_m: f32, 1.2),
    ("effects.blur", effects_blur: f32, 0.85),
    ("effects.blur_start_kmh", effects_blur_start_kmh: f32, 8.0),
    ("effects.blur_full_kmh", effects_blur_full_kmh: f32, 110.0),
    ("effects.fringe", effects_fringe: f32, 0.006),
    ("effects.grain", effects_grain: f32, 0.018),
    ("effects.grade", effects_grade: f32, 1.0),
    ("effects.grade_contrast", effects_grade_contrast: f32, 0.82),
    ("effects.grade_saturation", effects_grade_saturation: f32, 0.96),
    ("effects.grade_vignette", effects_grade_vignette: f32, 0.20),
    ("effects.grade_shadow_tint", effects_grade_shadow_tint: [f32; 3], [-0.045, 0.014, 0.032]),
    ("effects.grade_highlight_tint", effects_grade_highlight_tint: [f32; 3], [0.055, 0.018, -0.035]),
    ("effects.grade_exposure_ev", effects_grade_exposure_ev: f32, 0.10),
    ("effects.grade_temperature", effects_grade_temperature: f32, 0.16),
    ("effects.grade_tint", effects_grade_tint: f32, 0.0),
    ("effects.grade_shoulder", effects_grade_shoulder: f32, 0.60),
    ("effects.grade_black_lift", effects_grade_black_lift: f32, 0.008),
    ("effects.blur_start", effects_blur_start: f32, 0.35),
    ("effects.blur_length", effects_blur_length: f32, 0.14),
    ("effects.shake_hz", effects_shake_hz: f32, 9.0),
    ("effects.shadow", effects_shadow: f32, 0.55),
    ("effects.tire_marks", effects_tire_marks: f32, 1.0),
    ("effects.dust", effects_dust: f32, 1.0),
    ("effects.sun", effects_sun: [f32; 3], [0.35, 1.0, 0.25]),
    ("effects.level_sun", effects_level_sun: bool, true),
    ("effects.car_specular", effects_car_specular: f32, 1.0),
    ("effects.car_roughness", effects_car_roughness: f32, 0.32),
    ("effects.dof", effects_dof: f32, 1.0),
    ("effects.dof_focus_m", effects_dof_focus_m: f32, 0.0),
    ("effects.dof_range_m", effects_dof_range_m: f32, 25.0),
    ("effects.dof_radius_px", effects_dof_radius_px: f32, 8.0),
    ("effects.car_sharpen", effects_car_sharpen: f32, 0.85),
    ("effects.car_clarity", effects_car_clarity: f32, 0.18),
    ("effects.portal_frame", effects_portal_frame: f32, 1.0),
    ("effects.portal_width_px", effects_portal_width_px: f32, 18.0),
    ("effects.car_haze_cutoff", effects_car_haze_cutoff: f32, 0.02),
    // [hud]: the dashboard drawn over the frame.
    ("hud.enabled", hud_enabled: bool, true),
    ("hud.design", hud_design: String, "porsche-992-gt3rs".to_string()),
    // [game]: session defaults.
    ("game.start_mode", game_start_mode: String, "practice".to_string()),
    ("game.countdown_seconds", game_countdown_seconds: f32, 3.0),
    // [live]: frames written for a browser or an agent to view while the window runs.
    ("live.frame_dir", live_frame_dir: String, "logs/live".to_string()),
    ("live.interval_s", live_interval_s: f32, 1.0),
    // [console]: the agent console.
    ("console.enabled", console_enabled: bool, true),
    ("console.stdin", console_stdin: bool, true),
}

impl EngineConfig {
    /// Parses the file format. Every problem is reported with its line number, so one run shows them all.
    pub fn parse(text: &str) -> Result<Self, Vec<ConfigError>> {
        let mut config = Self::default();
        let mut errors = Vec::new();
        let mut section = String::new();
        for (index, raw) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.trim().to_string();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                errors.push(ConfigError { line: Some(line_no), message: format!("expected `key = value`, got `{line}`") });
                continue;
            };
            let path = format!("{}.{}", section, key.trim());
            if let Err(message) = config.set(&path, value.trim()) {
                errors.push(ConfigError { line: Some(line_no), message });
            }
        }
        if errors.is_empty() { Ok(config) } else { Err(errors) }
    }

    /// Writes every setting with its section header, for dumping the active configuration.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let mut section = "";
        for path in Self::PATHS {
            let (sec, key) = path.split_once('.').expect("paths are section.key");
            if sec != section {
                if !section.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[{sec}]\n"));
                section = sec;
            }
            let value = self.get(path).expect("declared path");
            out.push_str(&format!("{key} = {value}\n"));
        }
        out
    }
}

/// Removes a `#` or `;` comment. A comment marker only counts at the start of the line or after whitespace.
fn strip_comment(line: &str) -> &str {
    let mut previous_is_space = true;
    for (index, c) in line.char_indices() {
        if (c == '#' || c == ';') && previous_is_space {
            return &line[..index];
        }
        previous_is_space = c.is_whitespace();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_text() {
        let config = EngineConfig::default();
        let parsed = EngineConfig::parse(&config.to_text()).expect("dumped config parses");
        assert_eq!(parsed, config);
    }

    #[test]
    fn repository_engine_conf_parses_to_defaults_plus_overrides() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../engine.conf")).expect("engine.conf exists");
        let config = EngineConfig::parse(&text).expect("engine.conf is valid");
        assert_eq!(config.tick_hz, 60.0);
    }

    #[test]
    fn unknown_keys_and_bad_values_report_line_numbers() {
        let errors = EngineConfig::parse("[window]\nwidth = 1280\nwidht = 2\n[camera]\nfov_degrees = wide\n").unwrap_err();
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].line, Some(3));
        assert!(errors[0].message.contains("unknown setting `window.widht`"));
        assert_eq!(errors[1].line, Some(5));
    }

    #[test]
    fn vec3_accepts_spaces_or_commas_and_comments_are_stripped() {
        let config = EngineConfig::parse("[physics] # gravity\ngravity = 0,-5.5,0 ; down\n").expect("valid");
        assert_eq!(config.physics_gravity, [0.0, -5.5, 0.0]);
    }

    #[test]
    fn set_uses_the_same_parser_as_the_file() {
        let mut config = EngineConfig::default();
        config.set("hdri.exposure_is_not_a_key", "1").unwrap_err();
        config.set("render.exposure", "2.5").unwrap();
        assert_eq!(config.get("render.exposure").as_deref(), Some("2.5"));
        assert!(config.set("render.exposure", "bright").is_err());
    }
}
