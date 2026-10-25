//! The scene as entities. Every object in the world is an entity with components, and behaviour is systems that
//! read and write those components each tick.
//!
//! Entities created at start-up:
//! - `car`: a `Transform` that the physics world drives, a `Velocity`, a `Renderable` for the splat, and a `Vehicle`.
//! - `floor`: a `Transform` at the grid origin and a `Collider` for the level's voxel floor.
//! - `camera`: a `CameraRig` that follows the car or orbits the splat.
//!
//! Anything else is made at run time (`spawn marker`) and can be given a behaviour (`attach`) or removed (`despawn`).

use crate::camera::OrbitCamera;
use serde_json::{json, Value};
use splat_camera::{CarPose, FollowCamera, FollowParams};
use splat_ecs::{Entity, Schedule, World};
use splat_render::camera::Mat4;

/// Where an object is and how it is turned. Position is world space; rotation is a quaternion `[x, y, z, w]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

impl Default for Transform {
    fn default() -> Self {
        Self { position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0] }
    }
}

/// Linear velocity in metres per second, world space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Velocity(pub [f32; 3]);

/// A human-readable name, shown by the console.
#[derive(Debug, Clone, PartialEq)]
pub struct Name(pub String);

/// What the renderer draws for an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderable {
    /// The car's splat, with the body's pose applied.
    CarSplat,
}

/// A collision shape the physics world holds for this entity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Collider {
    /// The level's voxel floor, with the number of floor cells it was built from.
    VoxelFloor { cells: usize },
    /// The car's convex hull, with the number of occupied voxels it came from.
    VehicleHull { cells: usize },
}

/// Marks the car. `centroid` is the body origin in splat space after the model rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vehicle {
    pub centroid: [f32; 3],
    pub mass_kg: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    Follow,
    Orbit,
}

/// The camera: an orbit around the splat, and a chase camera behind a target entity. `mode` picks which one is seen.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraRig {
    pub orbit: OrbitCamera,
    pub follow: Option<FollowCamera>,
    pub mode: CameraMode,
    pub target: Option<Entity>,
}

impl CameraRig {
    /// The world-to-view matrix for the active mode.
    pub fn view(&self) -> Mat4 {
        match (self.mode, &self.follow) {
            (CameraMode::Follow, Some(follow)) => follow.view(),
            _ => self.orbit.view(),
        }
    }

    /// Whether the chase camera is the one being seen.
    pub fn following(&self) -> bool {
        self.mode == CameraMode::Follow && self.follow.is_some()
    }

    pub fn orbit_input(&mut self, dx: f32, dy: f32) {
        match (self.mode, self.follow.as_mut()) {
            (CameraMode::Follow, Some(follow)) => follow.orbit(dx, dy),
            _ => self.orbit.orbit(dx, dy),
        }
    }

    pub fn pan_input(&mut self, dx: f32, dy: f32) {
        match (self.mode, self.follow.as_mut()) {
            (CameraMode::Follow, Some(follow)) => follow.pan(dx, dy),
            _ => self.orbit.pan(dx, dy),
        }
    }

    /// Height: middle-drag. Moves the active camera up or down.
    pub fn raise_input(&mut self, dy: f32) {
        match (self.mode, self.follow.as_mut()) {
            (CameraMode::Follow, Some(follow)) => follow.raise(dy),
            _ => self.orbit.raise(dy),
        }
    }

    /// The active camera's eye and look point.
    pub fn eye_and_target(&self) -> ([f32; 3], [f32; 3]) {
        match (self.mode, &self.follow) {
            (CameraMode::Follow, Some(follow)) => (follow.eye(), follow.target()),
            _ => (self.orbit.eye(), self.orbit.target),
        }
    }

    pub fn zoom_input(&mut self, steps: f32) {
        match (self.mode, self.follow.as_mut()) {
            (CameraMode::Follow, Some(follow)) => follow.zoom(steps),
            _ => self.orbit.zoom(steps),
        }
    }
}

/// A behaviour an entity runs each tick. Attached and removed from the console.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Behaviour {
    /// Turns about the vertical axis at `deg_per_s`.
    Spin { deg_per_s: f32 },
    /// Moves up and down around `base_y` as a sine of `hz` cycles per second, with `amplitude` metres.
    Bob { amplitude: f32, hz: f32, base_y: f32 },
}

impl Behaviour {
    pub fn describe(&self) -> Value {
        match self {
            Behaviour::Spin { deg_per_s } => json!({ "kind": "spin", "deg_per_s": deg_per_s }),
            Behaviour::Bob { amplitude, hz, base_y } => json!({ "kind": "bob", "amplitude": amplitude, "hz": hz, "base_y": base_y }),
        }
    }
}

/// The clock every system reads: the fixed step and the tick count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    pub dt: f32,
    pub tick: u64,
}

/// The entities the engine always has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Core {
    pub car: Entity,
    pub floor: Entity,
    pub camera: Entity,
}

/// The systems, in order: behaviours move things, then the camera follows its target.
pub fn schedule() -> Schedule {
    let mut schedule = Schedule::new();
    schedule.add("behaviour", behaviour_system);
    schedule.add("camera", camera_system);
    schedule
}

fn behaviour_system(world: &mut World) {
    let clock = world.resource::<Clock>().copied().unwrap_or(Clock { dt: 0.0, tick: 0 });
    for entity in world.with::<Behaviour>() {
        let Some(behaviour) = world.get::<Behaviour>(entity).copied() else { continue };
        let Some(transform) = world.get_mut::<Transform>(entity) else { continue };
        match behaviour {
            Behaviour::Spin { deg_per_s } => {
                let half = (deg_per_s * clock.dt).to_radians() * 0.5;
                let (s, c) = half.sin_cos();
                transform.rotation = quat_mul([0.0, s, 0.0, c], transform.rotation);
            }
            Behaviour::Bob { amplitude, hz, base_y } => {
                let t = clock.tick as f32 * clock.dt;
                transform.position[1] = base_y + amplitude * (std::f32::consts::TAU * hz * t).sin();
            }
        }
    }
}

fn camera_system(world: &mut World) {
    let dt = world.resource::<Clock>().map_or(0.0, |c| c.dt);
    for camera in world.with::<CameraRig>() {
        let target = match world.get::<CameraRig>(camera) {
            Some(rig) if rig.following() => rig.target,
            _ => continue,
        };
        let Some(pose) = target.and_then(|t| pose_of(world, t)) else { continue };
        if let Some(follow) = world.get_mut::<CameraRig>(camera).and_then(|rig| rig.follow.as_mut()) {
            follow.update(&pose, dt);
        }
    }
}

/// The pose of an entity as the chase camera needs it: its transform and velocity.
pub fn pose_of(world: &World, entity: Entity) -> Option<CarPose> {
    let transform = world.get::<Transform>(entity)?;
    let velocity = world.get::<Velocity>(entity).map_or([0.0; 3], |v| v.0);
    Some(CarPose { position: transform.position, rotation: transform.rotation, velocity })
}

/// A chase camera with the default tuning, snapped behind `pose`.
/// A chase camera for the car. `forward_degrees` is where the car's nose points in its own frame, about Y from +Z
/// (`vehicle.forward_degrees`), so the camera sits behind the nose.
pub fn new_follow(pose: &CarPose, config: &crate::config::EngineConfig) -> FollowCamera {
    let params = FollowParams {
        distance: config.camera_follow_distance_m,
        height: config.camera_follow_height_m,
        look_height: config.camera_follow_look_height_m,
        position_damping: config.camera_follow_position_damping,
        look_damping: config.camera_follow_look_damping,
        velocity_heading_speed: config.camera_follow_velocity_heading_mps,
        heading_offset: config.vehicle_forward_degrees.to_radians(),
        reverse_swing: config.camera_reverse_swing,
        max_pitch: config.camera_follow_max_pitch_degrees.to_radians(),
    };
    FollowCamera::new(params, pose)
}

/// Hamilton product `a × b` of quaternions `[x, y, z, w]`.
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    let q = [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ];
    let len = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    q.map(|v| v / len)
}

/// The JSON for one entity: its id, name, components and behaviour, for `entity` and `entities`.
pub fn describe(world: &World, entity: Entity) -> Value {
    let name = world.get::<Name>(entity).map(|n| n.0.clone());
    let transform = world.get::<Transform>(entity).map(|t| json!({ "position": t.position, "rotation_xyzw": t.rotation }));
    let behaviour = world.get::<Behaviour>(entity).map(|b| b.describe());
    let components: Vec<&str> = world.component_names(entity).into_iter().map(short_name).collect();
    json!({ "id": entity.to_string(), "name": name, "transform": transform, "behaviour": behaviour, "components": components })
}

/// `splat_engine::scene::Transform` → `Transform`.
fn short_name(full: &str) -> &str {
    full.rsplit("::").next().unwrap_or(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spin_turns_about_the_vertical_axis() {
        let mut world = World::new();
        world.insert_resource(Clock { dt: 0.5, tick: 0 });
        let e = world.spawn();
        world.insert(e, Transform::default()).unwrap();
        world.insert(e, Behaviour::Spin { deg_per_s: 180.0 }).unwrap();
        let mut schedule = Schedule::new();
        schedule.add("behaviour", behaviour_system);
        schedule.run(&mut world);
        // 90 degrees about +Y: the quaternion's y component is sin(45°).
        let q = world.get::<Transform>(e).unwrap().rotation;
        assert!((q[1] - (std::f32::consts::FRAC_PI_4).sin()).abs() < 1e-5, "{q:?}");
        assert!((q[3] - (std::f32::consts::FRAC_PI_4).cos()).abs() < 1e-5);
    }

    #[test]
    fn bob_oscillates_around_its_base() {
        let mut world = World::new();
        world.insert_resource(Clock { dt: 1.0 / 60.0, tick: 15 });
        let e = world.spawn();
        world.insert(e, Transform { position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0] }).unwrap();
        world.insert(e, Behaviour::Bob { amplitude: 2.0, hz: 1.0, base_y: 5.0 }).unwrap();
        let mut schedule = Schedule::new();
        schedule.add("behaviour", behaviour_system);
        schedule.run(&mut world);
        // At t = 0.25 s the sine is at its peak.
        assert!((world.get::<Transform>(e).unwrap().position[1] - 7.0).abs() < 1e-4);
    }

    #[test]
    fn quaternion_product_composes_rotations() {
        let half = [0.0, (std::f32::consts::FRAC_PI_4).sin(), 0.0, (std::f32::consts::FRAC_PI_4).cos()];
        let full = quat_mul(half, half);
        assert!((full[1] - 1.0).abs() < 1e-5 && full[3].abs() < 1e-5, "{full:?}");
    }
}
