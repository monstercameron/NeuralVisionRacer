//! The rapier3d world (0.36.1, f32, dim3): a ground plane and the splat car as a dynamic body whose collider is
//! its own voxels. Stepping is fixed-rate: the engine calls [`Physics::step`] once per tick, which runs
//! `physics.substeps` rapier steps of `1 / (tick_hz × substeps)` seconds each.

use crate::config::EngineConfig;
use crate::drive::{Drivetrain, Geometry, Suspension};
use crate::voxel;
use splat_car::{CarSpec, Controls, Output};
use rapier3d::prelude::*;
use splat_game::vehicle::PhysicsSample as GamePhysicsSample;

/// A cuboid for one floor rectangle, positioned in the floor body's space.
fn floor_box(b: &crate::floor::FloorBox, voxel_size: f32) -> ColliderBuilder {
    let (w, d) = (b.w as f32 * voxel_size, b.d as f32 * voxel_size);
    let slab = crate::floor::SLAB_M;
    let centre = Vector::new(b.x as f32 * voxel_size + w / 2.0, b.top * voxel_size / voxel_size - slab / 2.0, b.z as f32 * voxel_size + d / 2.0);
    ColliderBuilder::cuboid(w / 2.0, slab / 2.0, d / 2.0).translation(centre)
}

/// How far below the ground plane the car may fall before it counts as out of bounds, in metres.
const BOUNDS_MARGIN: f32 = 5.0;

/// What the vehicle machine needs from one tick, as the physics world reports it.
pub type PhysicsSample = GamePhysicsSample;

/// The vehicle body and the offset from splat space to body space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehicleBody {
    pub body: RigidBodyHandle,
    pub collider: ColliderHandle,
    /// Splat-space centroid (after the model rotation). The body origin sits here, so the render transform is
    /// `translate(position) · rotate(body) · translate(-centroid) · model`.
    pub centroid: [f32; 3],
    pub occupied_voxels: usize,
    pub mass_kg: f32,
    pub voxel_size: f32,
}

/// A rigid body's pose: position and rotation quaternion as `[x, y, z, w]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyPose {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

pub struct Physics {
    pub world: PhysicsWorld,
    pub tire_marks: crate::tire_marks::TireTrails,
    pub dust: crate::dust::Dust,
    pub foliage: crate::foliage::Contact,
    foliage_drag: f32,
    foliage_cap: f32,
    substeps: u32,
    ground: Option<ColliderHandle>,
    vehicle: Option<VehicleBody>,
    /// The car's wheels and mechanics, when the vehicle was built with a car spec.
    drivetrain: Option<Drivetrain>,
    /// A level's collision, built in tiles around the car (`stream_collision`).
    level: Option<crate::collision::LevelCollision>,
    /// Play-area limits for `in_bounds`: half the ground extent in x and z, and the ground height.
    bounds_half_extent: f32,
    ground_y: f32,
    /// With a level, its voxel grid's box (min, max) replaces the flat-ground limits.
    level_bounds: Option<([f32; 3], [f32; 3])>,
    /// The car's heading at the spawn and on reset: a turn about +y, radians.
    spawn_yaw: f32,
}

impl Physics {
    pub fn new(config: &EngineConfig) -> Self {
        let mut world = PhysicsWorld::new();
        let [gx, gy, gz] = config.physics_gravity;
        world.gravity = Vector::new(gx, gy, gz);
        let substeps = config.physics_substeps.max(1);
        world.integration_parameters.dt = 1.0 / (config.tick_hz.max(1.0) * substeps as f32);
        Self {
            world,
            foliage: Default::default(), foliage_drag: config.foliage_drag_per_m, foliage_cap: config.foliage_max_decel_mps2,
            dust: {
                #[allow(unused_mut)] let mut dust=crate::dust::Dust::default();
                #[cfg(not(target_arch="wasm32"))] dust.set_clock(splat_render::metrics::clock_ms);
                dust
            },
            tire_marks: {
                #[allow(unused_mut)]
                let mut marks = crate::tire_marks::TireTrails::default();
                #[cfg(not(target_arch="wasm32"))]
                marks.set_clock(splat_render::metrics::clock_ms);
                marks
            },
            substeps,
            ground: None,
            vehicle: None,
            drivetrain: None,
            level: None,
            bounds_half_extent: config.physics_ground_half_extent,
            level_bounds: None,
            spawn_yaw: 0.0,
            ground_y: config.physics_ground_y,
        }
    }

    /// Adds the static ground plane (a thick cuboid whose top face is at `physics.ground_y`).
    pub fn add_ground(&mut self, config: &EngineConfig) -> ColliderHandle {
        let half = config.physics_ground_half_extent;
        let thickness = 0.5;
        let body = RigidBodyBuilder::fixed().translation(Vector::new(0.0, config.physics_ground_y - thickness, 0.0));
        let collider = ColliderBuilder::cuboid(half, thickness, half)
            .friction(config.physics_friction)
            .restitution(config.physics_restitution);
        let (_, handle) = self.world.insert(body, collider);
        self.ground = Some(handle);
        handle
    }

    /// The heading the car spawns and resets with: a turn about +y, radians.
    pub fn set_spawn_yaw(&mut self, yaw: f32) {
        self.spawn_yaw = yaw;
    }

    /// Uses `level`'s floor and walls as the ground, built only near the car: call `stream_collision` with the car's
    /// position (the engine does each tick).
    pub fn set_level(&mut self, level: &crate::level::LevelFloor, config: &EngineConfig) {
        self.tire_marks.clear();
        self.dust.clear();
        self.foliage=Default::default();
        let size = [0, 1, 2].map(|i| level.dims[i] as f32 * level.resolution);
        self.level_bounds = Some((level.origin, [0, 1, 2].map(|i| level.origin[i] + size[i])));
        self.level = Some(crate::collision::LevelCollision::new(level, config));
    }

    /// Builds the level's collision tiles around `centre` and drops far ones. Returns (built, dropped).
    pub fn stream_collision(&mut self, centre: [f32; 3]) -> (usize, usize) {
        let Some(level) = self.level.as_mut() else { return (0, 0) };
        level.update(&mut self.world, centre)
    }

    /// The streamed level collision's generation (changes when tiles are built or dropped) and its outline, world
    /// space, for the debug view; `None` without a level. See `LevelCollision::debug_segments`.
    pub fn collision_debug(&self, stride: usize) -> Option<(u64, Vec<([f32; 3], [f32; 3], bool)>)> {
        let level = self.level.as_ref()?;
        Some((level.generation(), level.debug_segments(&self.world, stride)))
    }

    pub fn collision_material_debug(&self,stride:usize)->Option<(u64,Vec<([f32;3],[f32;3],u8)>)> {
        let level=self.level.as_ref()?;
        Some((level.generation(),level.debug_material_segments(&self.world,stride)))
    }
    pub fn foliage_profile(&self)->serde_json::Value {
        let mut v=self.foliage.json();
        v["collision"]=self.level.as_ref().map_or(serde_json::Value::Null,|l|l.foliage_profile());v
    }

    /// The streamed level collision's generation alone (cheap), to tell whether `collision_debug` would change.
    pub fn collision_generation(&self) -> Option<u64> {
        self.level.as_ref().map(|l| l.generation())
    }

    pub fn collision_search_profile(&self) -> Option<serde_json::Value> {
        self.level.as_ref().map(|l| l.search_profile())
    }

    /// Collision tiles built now (0 without a level).
    pub fn collision_tiles(&self) -> usize {
        self.level.as_ref().map_or(0, |l| l.tiles())
    }

    pub fn road_profile(&self) -> Option<serde_json::Value> {
        self.level.as_ref().and_then(|l|l.road_profile())
    }

    /// Adds the whole level's floor as static boxes, built from the floor cells (the probes use this; the game
    /// streams tiles instead, see `set_level`). Each (x, z) column contributes the top of its
    /// floor cell as a height; columns with no floor are holes. Equal-height rectangles are merged into one cuboid
    /// each, with a slab of `floor::SLAB_M` underneath. `cells` are grid indices relative to `origin` (the grid
    /// minimum), `voxel_size` metres per cell, and `dims` the grid size in cells.
    ///
    /// Rapier's voxel shape gave no contacts against the car, and its heightfield gave no ray hits in this setup, while
    /// cuboids collide and query correctly. The boxes come straight from the voxel floor, so the surface is the same.
    pub fn add_voxel_floor(&mut self, cells: &[[u32; 3]], dims: [u32; 3], origin: [f32; 3], voxel_size: f32, config: &EngineConfig) -> Result<ColliderHandle, String> {
        if cells.is_empty() {
            return Err("no floor cells".into());
        }
        let (nx, nz) = (dims[0] as usize, dims[2] as usize);
        let mut heights: Vec<Option<f32>> = vec![None; nx * nz];
        for c in cells {
            let top = (c[1] as f32 + 1.0) * voxel_size;
            let slot = &mut heights[c[0] as usize * nz + c[2] as usize];
            *slot = Some(slot.map_or(top, |h: f32| h.max(top)));
        }
        let heights = crate::floor::smooth(&crate::floor::fill_holes(&heights, nx, nz), nx, nz);
        let boxes = crate::floor::merge(&heights, nx, nz);
        let body = RigidBodyBuilder::fixed().translation(Vector::new(origin[0], origin[1], origin[2]));
        let (body_handle, first) = {
            let b = boxes.first().ok_or("no floor boxes")?;
            let collider = floor_box(b, voxel_size).friction(config.physics_friction).restitution(config.physics_restitution);
            self.world.insert(body, collider)
        };
        for b in boxes.iter().skip(1) {
            let collider = floor_box(b, voxel_size).friction(config.physics_friction).restitution(config.physics_restitution);
            self.world.insert_collider(collider, Some(body_handle));
        }
        self.ground = Some(first);
        Ok(first)
    }

    /// Adds the level's walls: wall columns merged into rectangles that share a road height, each a cuboid from
    /// `floor::SLAB_M` below the road top up to `height_m` above it. `walls` is the block grid from `LevelFloor`.
    /// Returns the number of boxes.
    pub fn add_walls(&mut self, walls: &[Option<u32>], dims: [usize; 2], origin: [f32; 3], block_m: f32, height_m: f32, config: &EngineConfig) -> usize {
        let heights: Vec<Option<f32>> = walls.iter().map(|w| w.map(|r| r as f32)).collect();
        let boxes = crate::floor::merge(&heights, dims[0], dims[1]);
        if boxes.is_empty() {
            return 0;
        }
        let mut body = None;
        for b in &boxes {
            let (w, d) = (b.w as f32 * block_m, b.d as f32 * block_m);
            let road_top = (b.top + 1.0) * block_m;
            let bottom = road_top - crate::floor::SLAB_M;
            let top = road_top + height_m;
            let centre = Vector::new(b.x as f32 * block_m + w / 2.0, (bottom + top) / 2.0, b.z as f32 * block_m + d / 2.0);
            let collider = ColliderBuilder::cuboid(w / 2.0, (top - bottom) / 2.0, d / 2.0)
                .translation(centre)
                .friction(config.physics_friction)
                .restitution(config.physics_restitution);
            match body {
                Some(body) => {
                    self.world.insert_collider(collider, Some(body));
                }
                None => {
                    let fixed = RigidBodyBuilder::fixed().translation(Vector::new(origin[0], origin[1], origin[2]));
                    body = Some(self.world.insert(fixed, collider).0);
                }
            }
        }
        boxes.len()
    }

    /// Builds the vehicle: a dynamic body with one voxel collider covering the occupied splat cells.
    /// `spawn` is where the body's origin (the centroid) starts, in world space.
    ///
    /// Without a car spec the body slides on its hull, with `vehicle.mass_kg` spread over it. With one it becomes a
    /// car: four sprung wheels from the spec carry it (see `drive`), the hull starts above the tyres' contact patches
    /// so only the wheels touch the road, and the mass and centre of mass are the spec's (`vehicle.mass_kg` > 0
    /// overrides the mass).
    pub fn add_voxel_vehicle(&mut self, centres: &[[f32; 3]], config: &EngineConfig, spawn: [f32; 3], car: Option<&CarSpec>) -> Result<VehicleBody, String> {
        let voxel_size = config.physics_voxel_size;
        if voxel_size <= 0.0 {
            return Err("physics.voxel_size must be positive".into());
        }
        let centroid = voxel::centroid(centres);
        let cells = voxel::occupied_cells(centres, voxel_size, centroid);
        if cells.is_empty() {
            return Err("no splats to voxelise".into());
        }
        let spawn = Vector::new(spawn[0], spawn[1], spawn[2]);

        // The car's collider is the convex hull of its occupied voxel cells, in body space. Voxel-against-voxel
        // contacts with the floor were unreliable (the car sank through a voxel floor), while convex-against-voxel
        // contacts are stable. The hull is built from the cells' centres, so it covers the car's occupied volume.
        let body_centres: Vec<[f32; 3]> = centres.iter().map(|p| [p[0] - centroid[0], p[1] - centroid[1], p[2] - centroid[2]]).collect();
        let geometry = car.map(|spec| Geometry::fit(&body_centres, spec, config.vehicle_forward_degrees));
        let suspension = Suspension::from_config(config);
        let floor = geometry.map_or(f32::MIN, |g| g.bottom + suspension.hull_clearance_m);
        // A scanned car has stray splats around it, and a convex hull over them would be far wider than the car. With a
        // car spec the hull keeps only cells inside the splats' 0.5–99.5 percentile box.
        let bounds = car.map(|_| robust_bounds(&body_centres, 0.005));
        let inside = |c: &[i32; 3]| {
            bounds.is_none_or(|(lo, hi): ([f32; 3], [f32; 3])| {
                (0..3).all(|i| {
                    let v = (c[i] as f32 + 0.5) * voxel_size;
                    v >= lo[i] - voxel_size && v <= hi[i] + voxel_size
                })
            })
        };
        let hull_points: Vec<Vector> = cells
            .iter()
            .filter(|c| (c[1] as f32) * voxel_size >= floor && inside(c))
            .flat_map(|c| {
                let centre = |v: i32| (v as f32 + 0.5) * voxel_size;
                [Vector::new(centre(c[0]) - voxel_size / 2.0, centre(c[1]) - voxel_size / 2.0, centre(c[2]) - voxel_size / 2.0),
                 Vector::new(centre(c[0]) + voxel_size / 2.0, centre(c[1]) + voxel_size / 2.0, centre(c[2]) + voxel_size / 2.0)]
            })
            .collect();
        let hull = ColliderBuilder::convex_hull(&hull_points).ok_or("could not build the car's convex hull")?;
        let body = RigidBodyBuilder::dynamic().translation(spawn).rotation(Vector::new(0.0, self.spawn_yaw, 0.0)).ccd_enabled(config.physics_ccd);
        let mut spec = car.cloned();
        if let Some(spec) = spec.as_mut().filter(|_| config.vehicle_mass_kg > 0.0) {
            spec.mass_kg = config.vehicle_mass_kg;
        }
        let mass_kg = spec.as_ref().map_or(config.vehicle_mass_kg.max(1.0), |s| s.mass_kg);
        let collider = match (&spec, geometry) {
            (Some(spec), Some(geometry)) => {
                let extent = |axis: Vector| {
                    let values = hull_points.iter().map(|p| p.dot(axis));
                    let (lo, hi) = values.fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
                    hi - lo
                };
                let height = extent(Vector::Y) + suspension.hull_clearance_m;
                hull.mass_properties(geometry.mass_properties(spec, extent(geometry.right), height))
            }
            _ => hull.mass(mass_kg),
        };
        // A car's body slides: it should only touch walls and kerbs, and the tyres, not the body, grip the road. The
        // minimum combine rule keeps it slippery against the grippy floor (an average would give the floor's grip
        // back), so a car beached on a kerb can reverse off it.
        let collider = if spec.is_some() {
            collider.friction(config.vehicle_hull_friction).friction_combine_rule(CoefficientCombineRule::Min)
        } else {
            collider.friction(config.physics_friction)
        }
        .restitution(config.physics_restitution);
        let (handle, collider_handle) = self.world.insert(body, collider);
        let vehicle = VehicleBody {
            body: handle,
            collider: collider_handle,
            centroid,
            occupied_voxels: cells.len(),
            mass_kg,
            voxel_size,
        };
        self.vehicle = Some(vehicle);
        self.drivetrain = match (spec, geometry) {
            (Some(spec), Some(geometry)) => Some(Drivetrain::new(handle, spec, geometry, suspension)),
            _ => None,
        };
        Ok(vehicle)
    }

    /// Removes the vehicle body and its collider, if there is one.
    pub fn remove_vehicle(&mut self) {
        self.foliage=Default::default();
        self.tire_marks.break_tracks();
        self.dust.clear();
        if let Some(vehicle) = self.vehicle.take() {
            self.world.remove_body_with_colliders(vehicle.body, true);
        }
        self.drivetrain = None;
    }

    /// Holds the vehicle still (gravity off, velocity zero) or lets it move again.
    pub fn set_frozen(&mut self, frozen: bool) -> Result<(), String> {
        let vehicle = self.vehicle.ok_or("no vehicle")?;
        let body = self.world.bodies.get_mut(vehicle.body).ok_or("vehicle body missing")?;
        body.set_gravity_scale(if frozen { 0.0 } else { 1.0 }, true);
        if frozen {
            body.set_linvel(Vector::ZERO, true);
            body.set_angvel(Vector::ZERO, true);
        }
        Ok(())
    }

    /// One tick's observations of the vehicle, for the vehicle machine. `None` when there is no vehicle body.
    pub fn sample(&self) -> Option<PhysicsSample> {
        let vehicle = self.vehicle?;
        let body = self.world.bodies.get(vehicle.body)?;
        let mut contacts = 0u8;
        let mut impulse = 0.0f32;
        for pair in self.world.contact_pairs_with(vehicle.collider) {
            let other = if pair.collider1 == vehicle.collider { pair.collider2 } else { pair.collider1 };
            if other == vehicle.collider || !pair.has_any_active_contact() {
                continue;
            }
            contacts = contacts.saturating_add(1);
            impulse += pair.total_impulse_magnitude();
        }
        // A car stands on its wheels; the hull only touches walls (and the road in a crash).
        let hull_contacts = contacts;
        contacts = contacts.saturating_add(self.drivetrain.as_ref().map_or(0, |d| d.wheels_on_ground()));
        let t = body.translation();
        let half = self.bounds_half_extent;
        let in_bounds = match self.level_bounds {
            // Inside the level's grid in plan, and not fallen below it.
            Some((lo, hi)) => t.x >= lo[0] - BOUNDS_MARGIN && t.x <= hi[0] + BOUNDS_MARGIN && t.z >= lo[2] - BOUNDS_MARGIN && t.z <= hi[2] + BOUNDS_MARGIN && t.y >= lo[1] - BOUNDS_MARGIN,
            None => t.x.abs() <= half && t.z.abs() <= half && t.y >= self.ground_y - BOUNDS_MARGIN,
        };
        Some(PhysicsSample {
            ground_contacts: contacts,
            chassis_impact_mps: if hull_contacts > 0 {
                Some(impulse / vehicle.mass_kg)
            } else if contacts > 0 {
                Some(0.0)
            } else {
                None
            },
            speed_mps: body.linvel().length(),
            in_bounds,
        })
    }

    /// Advances the world by one engine tick.
    pub fn step(&mut self) {
        let dt = self.world.integration_parameters.dt;
        for _ in 0..self.substeps {
            if let Some(drivetrain) = self.drivetrain.as_mut() {
                drivetrain.update(&mut self.world, dt);
            }
            let contacts = self.drivetrain.as_ref().map_or([None; 4], |d| d.tire_contacts(&self.world));
            self.tire_marks.step(contacts, dt);
            self.dust.step(contacts, dt);
            self.world.step();
            if let Some(v)=self.vehicle {
                self.foliage.step(&mut self.world,v.body,v.collider,dt,self.foliage_drag,self.foliage_cap);
            }
        }
    }

    /// Sets the driver's controls; they stay until changed. Ignored without a car.
    pub fn set_controls(&mut self, controls: Controls) {
        if let Some(drivetrain) = self.drivetrain.as_mut() {
            drivetrain.controls = controls;
        }
    }

    /// How far the car's lowest point (its tyres' contact) is below the body origin, in body space; `None` without
    /// a wheeled car.
    pub fn car_bottom(&self) -> Option<f32> {
        self.drivetrain.as_ref().map(|d| d.geometry().bottom)
    }

    /// Per wheel: on the ground, and suspension length. Empty without a car.
    pub fn wheel_states(&self) -> Vec<(bool, f32)> {
        self.drivetrain.as_ref().map_or_else(Vec::new, |d| d.wheel_states())
    }

    pub fn visual_wheels(&self) -> [crate::drive::WheelVisualState; 4] {
        self.drivetrain.as_ref().map_or([Default::default(); 4], |d| d.visual_wheels())
    }

    pub fn wheel_geometry(&self) -> Option<Geometry> {
        self.drivetrain.as_ref().map(|d| d.geometry())
    }

    /// The current controls (all zero without a car).
    pub fn controls(&self) -> Controls {
        self.drivetrain.as_ref().map_or_else(Controls::default, |d| d.controls)
    }

    /// The car's mechanics: spec, gear, auto/manual. `None` without a car.
    pub fn car(&self) -> Option<&splat_car::Car> {
        self.drivetrain.as_ref().map(|d| &d.car)
    }

    pub fn car_mut(&mut self) -> Option<&mut splat_car::Car> {
        self.drivetrain.as_mut().map(|d| &mut d.car)
    }

    /// The last step's driveline output (rpm, gear, forces). `None` without a car.
    pub fn car_output(&self) -> Option<Output> {
        self.drivetrain.as_ref().map(|d| *d.car.last())
    }

    /// Puts the vehicle back at `position` with no velocity, as the console `reset` does.
    pub fn reset_vehicle(&mut self, position: [f32; 3]) -> Result<(), String> {
        self.foliage=Default::default();
        self.tire_marks.break_tracks();
        self.dust.clear();
        let vehicle = self.vehicle.ok_or("no vehicle")?;
        let body = self.world.bodies.get_mut(vehicle.body).ok_or("vehicle body missing")?;
        body.set_position(Pose::new(Vector::new(position[0], position[1], position[2]), Vector::new(0.0, self.spawn_yaw, 0.0)), true);
        body.set_linvel(Vector::ZERO, true);
        body.set_angvel(Vector::ZERO, true);
        if let Some(drivetrain) = self.drivetrain.as_mut() {
            drivetrain.car.reset();
            drivetrain.reset_rotation();
        }
        Ok(())
    }

    /// Rights the vehicle where it is (anti-flip): upright, facing the way it faced, `lift_m` higher, at rest. The
    /// gear and engine state stay. Returns the new position.
    pub fn right_vehicle(&mut self, lift_m: f32) -> Result<[f32; 3], String> {
        self.tire_marks.break_tracks();
        self.dust.clear();
        let vehicle = self.vehicle.ok_or("no vehicle")?;
        let body = self.world.bodies.get_mut(vehicle.body).ok_or("vehicle body missing")?;
        // The heading of the body's +z (the spawn yaw's reference), flattened; straight up or down keeps the spawn's.
        let ahead = *body.rotation() * Vector::Z;
        let yaw = if ahead.x.hypot(ahead.z) > 0.1 { ahead.x.atan2(ahead.z) } else { self.spawn_yaw };
        let p = body.translation() + Vector::new(0.0, lift_m, 0.0);
        body.set_position(Pose::new(p, Vector::new(0.0, yaw, 0.0)), true);
        body.set_linvel(Vector::ZERO, true);
        body.set_angvel(Vector::ZERO, true);
        if let Some(drive) = self.drivetrain.as_mut() { drive.reset_rotation(); }
        Ok([p.x, p.y, p.z])
    }

    pub fn vehicle(&self) -> Option<VehicleBody> {
        self.vehicle
    }

    pub fn ground(&self) -> Option<ColliderHandle> {
        self.ground
    }

    /// Whether there is ground: a plane, a whole floor, or streamed tiles around the car.
    pub fn has_ground(&self) -> bool {
        self.ground.is_some() || self.level.as_ref().is_some_and(|l| l.has_ground())
    }

    /// The vehicle's pose, or `None` when there is no vehicle.
    pub fn vehicle_pose(&self) -> Option<BodyPose> {
        let vehicle = self.vehicle?;
        let body = self.world.bodies.get(vehicle.body)?;
        let t = body.translation();
        let r = body.rotation();
        Some(BodyPose { position: [t.x, t.y, t.z], rotation: [r.x, r.y, r.z, r.w] })
    }

    /// The vehicle's linear velocity in m/s, world space. Zero when there is no vehicle.
    pub fn vehicle_velocity(&self) -> [f32; 3] {
        self.vehicle
            .and_then(|v| self.world.bodies.get(v.body))
            .map_or([0.0; 3], |b| {
                let v = b.linvel();
                [v.x, v.y, v.z]
            })
    }

    /// The vehicle's linear speed in m/s.
    pub fn vehicle_speed(&self) -> f32 {
        self.vehicle
            .and_then(|v| self.world.bodies.get(v.body))
            .map_or(0.0, |b| b.linvel().length())
    }
}

/// Per-axis `q` and `1 - q` percentiles of `points`: a box that ignores the stray few.
fn robust_bounds(points: &[[f32; 3]], q: f32) -> ([f32; 3], [f32; 3]) {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for axis in 0..3 {
        let mut values: Vec<f32> = points.iter().map(|p| p[axis]).collect();
        values.sort_by(f32::total_cmp);
        let at = |f: f32| values.get(((values.len().max(1) - 1) as f32 * f) as usize).copied().unwrap_or(0.0);
        lo[axis] = at(q);
        hi[axis] = at(1.0 - q);
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_points(side: usize, spacing: f32) -> Vec<[f32; 3]> {
        let mut points = Vec::new();
        for x in 0..side {
            for y in 0..side {
                for z in 0..side {
                    points.push([x as f32 * spacing, y as f32 * spacing, z as f32 * spacing]);
                }
            }
        }
        points
    }

    #[test]
    fn a_dropped_voxel_body_comes_to_rest_on_the_ground() {
        let mut config = EngineConfig::default();
        config.vehicle_spawn = [0.0, 1.5, 0.0];
        config.physics_voxel_size = 0.1;
        let points = cube_points(6, 0.1);
        let mut physics = Physics::new(&config);
        physics.add_ground(&config);
        let vehicle = physics.add_voxel_vehicle(&points, &config, config.vehicle_spawn, None).expect("vehicle");
        assert_eq!(vehicle.occupied_voxels, 216);

        for _ in 0..600 {
            physics.step();
        }
        let pose = physics.vehicle_pose().expect("pose");
        // The cube is 0.6 m on a side and its centroid is at the middle, so it rests with its centre at 0.3 m.
        assert!((pose.position[1] - 0.3).abs() < 0.02, "rest height {}", pose.position[1]);
        assert!(physics.vehicle_speed() < 0.05, "still moving at {}", physics.vehicle_speed());
    }

    #[test]
    fn reset_moves_the_vehicle_and_clears_velocity() {
        let mut config = EngineConfig::default();
        config.physics_ground = false;
        let mut physics = Physics::new(&config);
        physics.add_voxel_vehicle(&cube_points(3, 0.1), &config, config.vehicle_spawn, None).unwrap();
        for _ in 0..30 {
            physics.step();
        }
        physics.reset_vehicle([5.0, 10.0, -2.0]).unwrap();
        let pose = physics.vehicle_pose().unwrap();
        assert_eq!(pose.position, [5.0, 10.0, -2.0]);
        assert_eq!(physics.vehicle_speed(), 0.0);
    }
}
