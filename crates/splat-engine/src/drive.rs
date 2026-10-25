//! The car on the road: a ray-cast vehicle (`wheels`: rapier's, with footprint wheels and skyhook damping for the
//! scanned roads; suspension, tyre grip, steering) driven by `splat-car`'s
//! mechanics (engine, gearbox, brakes, aero).
//!
//! Four wheels hang from the chassis at the spec's wheelbase and track. Each physics substep the mechanics turn the
//! controls and the car's forward speed into a drive force at the driven wheels, brake forces per axle and aero
//! loads; the wheels' grip is the spec's tyre µ against the suspension load, so the traction limit, load transfer
//! and downforce all come through the tyres.

use crate::wheels::{DynamicRayCastVehicleController, WheelTuning};
use rapier3d::prelude::*;
use splat_car::{Car, CarSpec, Controls, Drive, Output};

/// Suspension and chassis tuning, from `[vehicle]` in engine.conf.
///
/// - `sag_m`: static sag; the springs are sized so each axle settles this far under its share of the weight (7 cm is
///   a ride frequency of about 1.9 Hz). Rapier's springs carry no preload, so a wheel also leaves the road once it
///   droops this far, under load transfer at launch or over a crest.
/// - `rest_m`: suspension length with the spring unloaded; the static length is `rest_m − sag_m`.
/// - `damping_compression`, `damping_rebound`: damping as a share of critical.
/// - `anti_squat`, `anti_dive`: the share of the pitching moment from driving and braking that the suspension
///   geometry carries instead of the springs; without it hard launches lift the front wheels off cobbles.
/// - `hull_clearance_m`: the chassis collider starts this far above the tyres' contact patches, so only the wheels
///   touch the road and the body clears kerbs.
/// - `wall_slide`: 0..1, how much a car touching a wall is helped to glance off it: turned towards the wall's line
///   and its tyres' sideways grip eased, so it slides along instead of scrubbing to a stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Suspension {
    pub sag_m: f32,
    pub rest_m: f32,
    pub damping_compression: f32,
    pub damping_rebound: f32,
    pub anti_squat: f32,
    pub anti_dive: f32,
    pub hull_clearance_m: f32,
    pub wall_slide: f32,
}

impl Suspension {
    pub fn from_config(config: &crate::config::EngineConfig) -> Self {
        Self {
            sag_m: config.vehicle_suspension_sag_m.max(0.005),
            rest_m: config.vehicle_suspension_rest_m.max(config.vehicle_suspension_sag_m + 0.01),
            damping_compression: config.vehicle_damping_compression,
            damping_rebound: config.vehicle_damping_rebound,
            anti_squat: config.vehicle_anti_squat,
            anti_dive: config.vehicle_anti_dive,
            hull_clearance_m: config.vehicle_hull_clearance_m.max(0.0),
            wall_slide: config.vehicle_wall_slide.clamp(0.0, 1.0),
        }
    }
}

/// The car's layout in body space (origin at the splat centroid), from the splat and the spec.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// Unit forward and right axes.
    pub forward: Vector,
    pub right: Vector,
    /// Lowest point of the car (tyre contact) along up.
    pub bottom: f32,
    /// Front and rear axle positions along `forward`.
    pub front_axle: f32,
    pub rear_axle: f32,
}

impl Geometry {
    /// Places the axles from the nose (the 99.5th percentile of the splat along `forward_degrees`) using the spec's
    /// front overhang and wheelbase. `centres` are body-space splat centres.
    pub fn fit(centres: &[[f32; 3]], spec: &CarSpec, forward_degrees: f32) -> Self {
        let (s, c) = forward_degrees.to_radians().sin_cos();
        let forward = Vector::new(s, 0.0, c);
        let right = Vector::new(-c, 0.0, s);
        let mut along: Vec<f32> = centres.iter().map(|p| p[0] * forward.x + p[2] * forward.z).collect();
        along.sort_by(f32::total_cmp);
        let mut heights: Vec<f32> = centres.iter().map(|p| p[1]).collect();
        heights.sort_by(f32::total_cmp);
        let at = |v: &[f32], q: f32| v.get(((v.len().max(1) - 1) as f32 * q) as usize).copied().unwrap_or(0.0);
        let nose = at(&along, 0.995);
        let front_axle = nose - spec.front_overhang_m;
        Self { forward, right, bottom: at(&heights, 0.002), front_axle, rear_axle: front_axle - spec.wheelbase_m }
    }

    /// Body-space centre of mass: along the wheelbase by the weight split, at the spec's CoG height.
    pub fn centre_of_mass(&self, spec: &CarSpec) -> Vector {
        let along = self.rear_axle + spec.wheelbase_m * spec.weight_front;
        self.forward * along + Vector::new(0.0, self.bottom + spec.cog_height_m, 0.0)
    }

    /// Mass properties for the chassis: the spec's mass at its centre of mass, with a box's inertia over the car's
    /// length, width and height, turned to the car's forward axis.
    pub fn mass_properties(&self, spec: &CarSpec, width: f32, height: f32) -> MassProperties {
        let m = spec.mass_kg;
        let (l, w, h) = (spec.length_m, width, height);
        // Principal axes in the car's frame: x across (pitch), y up (yaw), z along (roll).
        let inertia = Vector::new(m / 12.0 * (h * h + l * l), m / 12.0 * (w * w + l * l), m / 12.0 * (w * w + h * h));
        let frame = Rotation::from_rotation_arc(Vector::Z, self.forward);
        MassProperties::with_principal_inertia_frame(self.centre_of_mass(spec), m, inertia, frame)
    }
}

/// A contact counts as a wall when its normal's vertical part is below this (steeper than about 60°).
const WALL_STEEPNESS: f32 = 0.5;
/// Yaw rate per radian of misalignment that turns a car touching a wall towards the wall's line, rad/s.
const WALL_TURN_RATE: f32 = 4.0;

/// Wheels, mechanics and the latest controls for the one car.
pub struct Drivetrain {
    pub car: Car,
    pub controls: Controls,
    controller: DynamicRayCastVehicleController,
    geometry: Geometry,
    suspension: Suspension,
    /// Per wheel: front axle?
    front: [bool; 4],
}

impl Drivetrain {
    /// Hangs four wheels on `chassis` for `spec`, placed by `geometry`.
    pub fn new(chassis: RigidBodyHandle, spec: CarSpec, geometry: Geometry, suspension: Suspension) -> Self {
        let s = suspension;
        let mut controller = DynamicRayCastVehicleController::new(chassis);
        let front = [true, true, false, false];
        for (i, &is_front) in front.iter().enumerate() {
            let side = if i % 2 == 0 { -1.0 } else { 1.0 };
            let (axle, track, radius, share) = if is_front {
                (geometry.front_axle, spec.track_front_m, spec.front_radius_m, spec.weight_front / 2.0)
            } else {
                (geometry.rear_axle, spec.track_rear_m, spec.rear_radius_m, (1.0 - spec.weight_front) / 2.0)
            };
            // Spring: this wheel's share of the weight compresses it by the sag. Rapier's force is k·compression·mass.
            let stiffness = share * 9.81 / s.sag_m;
            let critical = 2.0 * (stiffness * share).sqrt();
            let tuning = WheelTuning {
                suspension_stiffness: stiffness,
                suspension_compression: s.damping_compression * critical,
                suspension_damping: s.damping_rebound * critical,
                max_suspension_travel: s.rest_m,
                side_friction_stiffness: 1.0,
                friction_slip: spec.grip_mu,
                max_suspension_force: spec.mass_kg * 9.81 * 3.0,
            };
            // At the static sag the tyre touches the car's lowest point, so the splat sits on the road.
            let centre = geometry.forward * axle + geometry.right * (side * track / 2.0) + Vector::new(0.0, geometry.bottom + radius, 0.0);
            let hardpoint = centre + Vector::new(0.0, s.rest_m - s.sag_m, 0.0);
            controller.add_wheel(hardpoint, Vector::new(0.0, -1.0, 0.0), geometry.right, s.rest_m, radius, &tuning);
        }
        Self { car: Car::new(spec), controls: Controls::default(), controller, geometry, suspension, front }
    }

    /// Runs the mechanics for one substep and hands the forces to the wheels and the chassis, then updates the
    /// suspension and tyres. Call before each `world.step()`.
    pub fn update(&mut self, world: &mut PhysicsWorld, dt: f32) -> Output {
        let chassis = self.controller.chassis;
        let Some(body) = world.bodies.get(chassis) else { return *self.car.last() };
        let rotation = *body.rotation();
        let forward = rotation * self.geometry.forward;
        let up = rotation * Vector::Y;
        let velocity = body.linvel();
        let speed = velocity.dot(forward);
        let out = self.car.step(self.controls, speed, dt);

        let driven = |front: bool| match self.car.spec.drive {
            Drive::Rwd => !front,
            Drive::Fwd => front,
            Drive::Awd => true,
        };
        let driven_count = self.front.iter().filter(|&&f| driven(f)).count().max(1) as f32;
        let braking = out.brake_front_n + out.brake_rear_n > 0.0;
        for (wheel, &front) in self.controller.wheels_mut().iter_mut().zip(&self.front) {
            // Rapier turns the wheel the other way to this crate's convention (positive = right).
            wheel.steering = if front { -out.steer_angle } else { 0.0 };
            // Rapier only applies a wheel's brake when its engine force is zero, so braking takes over the wheel.
            wheel.engine_force = if driven(front) && !braking { out.drive_force_n / driven_count } else { 0.0 };
            let axle_brake = if front { out.brake_front_n } else { out.brake_rear_n };
            wheel.brake = axle_brake / 2.0 * dt;
        }

        // Drag and rolling resistance oppose the motion; downforce presses the car down. Impulses, as forces would persist.
        if let Some(body) = world.bodies.get_mut(chassis) {
            let along = speed.signum();
            let mut impulse = -forward * (out.rolling_n * along) - up * out.downforce_n;
            if velocity.length() > 0.01 {
                impulse -= velocity.normalize() * out.drag_n;
            }
            let moving = out.drive_force_n != 0.0 || velocity.length() > 0.01;
            body.apply_impulse(impulse * dt, moving);
            // Driving pitches the nose up (a positive turn about the right axis) and braking pitches it down, by force ×
            // centre-of-gravity height; the anti-squat and anti-dive share is taken back here.
            let right = rotation * self.geometry.right;
            let brake = (out.brake_front_n + out.brake_rear_n) * along;
            let pitch = if braking { brake * self.suspension.anti_dive } else { -out.drive_force_n * self.suspension.anti_squat };
            body.apply_torque_impulse(right * (pitch * self.car.spec.cog_height_m * dt), moving);
        }

        // Glancing off walls: while the body touches something steep (a wall, a post), turn the car towards the wall's
        // line, whichever way along it the car already points, and ease the tyres' sideways grip, so it slides along
        // instead of scrubbing to a stop with its nose in the wall.
        let wall = self.wall_normal(world);
        let side_grip = if wall.is_some() { 1.0 - 0.6 * self.suspension.wall_slide } else { 1.0 };
        for wheel in self.controller.wheels_mut() {
            wheel.side_friction_stiffness = side_grip;
        }
        if let (Some(normal), Some(body)) = (wall, world.bodies.get_mut(chassis)) {
            let ahead = Vector::new(forward.x, 0.0, forward.z).normalize_or_zero();
            let mut along = Vector::new(-normal.z, 0.0, normal.x).normalize_or_zero();
            if along.dot(ahead) < 0.0 {
                along = -along;
            }
            // Signed angle about +y from the heading to the wall's line, eased in as a yaw rate.
            let angle = ahead.cross(along).y.atan2(ahead.dot(along));
            let mut spin = body.angvel();
            spin.y += (angle * WALL_TURN_RATE - spin.y) * self.suspension.wall_slide * (dt * 20.0).min(1.0);
            body.set_angvel(spin, true);
        }

        let filter = QueryFilter::default().exclude_rigid_body(chassis);
        let queries = world.broad_phase.as_query_pipeline_mut(world.narrow_phase.query_dispatcher(), &mut world.bodies, &mut world.colliders, filter);
        self.controller.update_vehicle(dt, queries);
        out
    }

    /// The horizontal direction out of a wall the body is touching, if any: the average normal of its contacts whose
    /// surface is steeper than `WALL_STEEPNESS` (so kerbs and the road do not count).
    fn wall_normal(&self, world: &PhysicsWorld) -> Option<Vector> {
        let body = world.bodies.get(self.controller.chassis)?;
        let mut sum = Vector::ZERO;
        for &collider in body.colliders() {
            for pair in world.contact_pairs_with(collider) {
                if !pair.has_any_active_contact() {
                    continue;
                }
                // Manifold normals point from the first collider to the second; turn them to point at the car.
                let flip = if pair.collider1 == collider { -1.0 } else { 1.0 };
                for manifold in pair.manifolds() {
                    let n = manifold.data.normal * flip;
                    if n.y.abs() < WALL_STEEPNESS {
                        sum += Vector::new(n.x, 0.0, n.z);
                    }
                }
            }
        }
        (sum.length() > 1e-3).then(|| sum.normalize())
    }

    /// Per wheel (front left, front right, rear left, rear right): touching the ground, and its suspension length.
    pub fn wheel_states(&self) -> Vec<(bool, f32)> {
        self.controller.wheels().iter().map(|w| (w.raycast_info().is_in_contact, w.raycast_info().suspension_length)).collect()
    }

    /// Wheels touching the ground.
    pub fn wheels_on_ground(&self) -> u8 {
        self.controller.wheels().iter().filter(|w| w.raycast_info().is_in_contact).count() as u8
    }

    /// Body-space wheel centres at rest, for debugging and tests.
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }
}
