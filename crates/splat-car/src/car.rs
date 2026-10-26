//! The running car: engine speed, gear and the forces the driveline puts on the road each step.
//!
//! The model is longitudinal. The physics engine owns the body, the suspension and the tyres' sideways grip;
//! this module turns pedals into a drive force at the driven wheels, brake forces per axle, and aero, from the spec:
//!
//! - **Engine**: torque from the full-throttle curve × throttle, less friction (engine braking) on a closed throttle.
//!   Fuel is cut at the limiter.
//! - **Clutch**: below idle in gear, or on a launch, the clutch slips and the engine holds its own speed (launch
//!   control holds `launch_rpm` at full throttle). Otherwise engine speed follows the wheels through the gearing.
//! - **Gearbox**: a PDK. In auto it upshifts at `upshift_rpm` and downshifts at `downshift_rpm` (or on kickdown);
//!   each shift cuts drive for `shift_time_s`. Shift requests (paddles) also work, in either mode.
//! - **Traction**: the drive force is capped by the driven axle's grip, with load transfer under acceleration and the
//!   axle's share of downforce. Exceeding it is reported as wheelspin.
//! - **Inertia**: the engine's rotating mass makes the car accelerate as if heavier in low gears. The physics body
//!   has only the car's mass, so the reported drive force is reduced to give the same acceleration.
//! - **Reverse**: arcade style. Holding the brake at a standstill selects reverse, where the brake pedal drives
//!   backwards and the throttle brakes; the throttle at a standstill selects first again.

use crate::spec::{CarSpec, Drive};

const G: f32 = 9.81;
/// Numerical rest only. The vehicle driver settles tiny braked motion to zero before selecting direction.
const STOPPED_MPS: f32 = 0.001;

/// Driver inputs for one step.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Controls {
    /// Forward request (W), 0..1; brakes reverse motion before selecting forward at rest.
    pub throttle: f32,
    /// Reverse request (S), 0..1; brakes forward motion before selecting reverse at rest.
    pub brake: f32,
    /// Rear-wheel handbrake, independent of travel direction, 0..1.
    pub handbrake: f32,
    /// -1 (left) .. 1 (right).
    pub steer: f32,
    /// +1 to shift up, -1 to shift down, 0 to leave it (paddles).
    pub shift: i8,
}

/// What the driveline does this step. Forces are in newtons along the car's forward axis unless stated.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Output {
    /// Total force at the driven wheels' contact patches. Negative drives backwards (reverse) or is engine braking.
    pub drive_force_n: f32,
    /// Signed axle torque before the road traction cap, for independent wheel rotation.
    pub axle_torque_nm: f32,
    /// Signed gear × final-drive ratio used for this step (before an automatic shift).
    pub drive_ratio: f32,
    /// Brake force at each axle, opposing the motion (positive).
    pub brake_front_n: f32,
    pub brake_rear_n: f32,
    pub handbrake_rear_n: f32,
    /// Resolved pedals after direction gating, for HUD/telemetry rather than raw W/S requests.
    pub throttle: f32,
    pub brake: f32,
    pub handbrake: f32,
    /// Aerodynamic drag, opposing the motion (positive).
    pub drag_n: f32,
    /// Rolling resistance, opposing the motion (positive).
    pub rolling_n: f32,
    /// Downforce, pressing the car onto the road (positive).
    pub downforce_n: f32,
    /// Front wheel angle in radians, positive to the right.
    pub steer_angle: f32,
    pub rpm: f32,
    /// -1 reverse, 0 neutral, 1.. forward gears.
    pub gear: i32,
    pub engine_torque_nm: f32,
    pub power_kw: f32,
    pub speed_mps: f32,
    pub shifting: bool,
    pub limiter: bool,
    pub wheelspin: bool,
    /// Requested drive beyond the axle's traction limit, before capping (0 means within grip).
    /// A torque-demand proxy, not measured tire slip.
    pub traction_excess: f32,
    pub launch: bool,
    /// Coolant and oil temperature (°C), oil pressure (bar) and boost (bar of gauge pressure; 0 without a turbo).
    pub water_temp_c: f32,
    pub oil_temp_c: f32,
    pub oil_pressure_bar: f32,
    pub boost_bar: f32,
}

#[derive(Debug, Clone)]
pub struct Car {
    pub spec: CarSpec,
    /// Auto-shifting on (the default), or manual.
    pub auto: bool,
    rpm: f32,
    gear: i32,
    /// Seconds left in the current shift's torque cut.
    shift_left: f32,
    /// Coolant and oil temperature, °C; boost, bar.
    water_c: f32,
    oil_c: f32,
    boost: f32,
    last: Output,
}


impl Car {
    pub fn new(spec: CarSpec) -> Self {
        let rpm = spec.idle_rpm;
        let (water_c, oil_c) = (spec.start_water_c, spec.start_oil_c);
        Self { spec, auto: true, rpm, gear: 1, shift_left: 0.0, water_c, oil_c, boost: 0.0, last: Output::default() }
    }

    /// The last step's output.
    pub fn last(&self) -> &Output {
        &self.last
    }

    /// The vehicle controller can replace the aggregate traction-demand flag with measured wheel slip.
    pub fn report_wheelspin(&mut self, slipping: bool) { self.last.wheelspin = slipping; }

    /// Back to first gear at idle, as after a respawn.
    pub fn reset(&mut self) {
        self.rpm = self.spec.idle_rpm;
        self.gear = 1;
        self.shift_left = 0.0;
        self.boost = 0.0;
        self.last = Output::default();
    }

    fn pedals(&self, controls: Controls, speed: f32) -> (f32, f32) {
        let w = controls.throttle.clamp(0.0, 1.0);
        let s = controls.brake.clamp(0.0, 1.0);
        // Both direction keys mean brake; they never select a direction or feed engine torque.
        if w > 0.05 && s > 0.05 { return (0.0, w.max(s)); }
        if w > 0.05 {
            if speed < -STOPPED_MPS || self.gear < 0 { (0.0, w) } else { (w, 0.0) }
        } else if s > 0.05 {
            if speed > STOPPED_MPS || self.gear >= 0 { (0.0, s) } else { (s, 0.0) }
        } else { (0.0, 0.0) }
    }

    /// Whether the direction keys are currently asking to stop, before direction selection.
    pub fn braking_requested(&self, controls: Controls, speed: f32) -> bool { self.pedals(controls, speed).1 > 0.0 }

    /// Advances by `dt` seconds. `speed_mps` is the car's forward speed (negative when rolling backwards).
    pub fn step(&mut self, controls: Controls, speed_mps: f32, dt: f32) -> Output {
        let spec = self.spec.clone();
        let throttle_in = controls.throttle.clamp(0.0, 1.0);
        let brake_in = controls.brake.clamp(0.0, 1.0);
        let stopped = speed_mps.abs() < STOPPED_MPS;

        // Direction: hold the brake at a standstill for reverse; the throttle at a standstill goes back to first.
        if stopped && self.gear >= 0 && brake_in > 0.1 && throttle_in < 0.05 {
            self.begin_shift(-1);
        } else if stopped && self.gear < 0 && throttle_in > 0.1 && brake_in < 0.05 {
            self.begin_shift(1);
        }
        let (throttle, brake) = self.pedals(controls, speed_mps);
        let handbrake = if controls.handbrake.is_finite() { controls.handbrake.clamp(0.0, 1.0) } else { 0.0 };

        // A request held across several steps shifts once: requests during a shift are ignored.
        if controls.shift != 0 && self.shift_left <= 0.0 {
            let target = (self.gear + i32::from(controls.shift)).clamp(1, spec.ratios.len() as i32);
            if self.gear > 0 && target != self.gear && self.rpm_in(target, speed_mps) < spec.limiter_rpm {
                self.begin_shift(target);
            }
        }

        let ratio = self.ratio();
        let wheel_rpm = speed_mps.abs() / spec.driven_radius_m() * 60.0 / std::f32::consts::TAU;
        let coupled = wheel_rpm * ratio * spec.final_drive;
        // The clutch slips when the wheels would pull the engine below idle, and on a launch.
        // Launch control lets the clutch slip at `launch_rpm` from a standstill and fades it out by `clutch_lock_mps`.
        let fade = (1.0 - speed_mps.abs() / spec.clutch_lock_mps.max(0.1)).clamp(0.0, 1.0);
        let launch_rpm = spec.idle_rpm + (spec.launch_rpm - spec.idle_rpm) * throttle;
        let launch = ratio > 0.0 && throttle > 0.5 && fade > 0.0 && coupled < launch_rpm && self.gear.abs() == 1;
        let slipping = ratio > 0.0 && (coupled < spec.idle_rpm || launch);
        let target_rpm = if ratio == 0.0 {
            spec.idle_rpm + (spec.limiter_rpm - spec.idle_rpm) * throttle
        } else if launch {
            // The clutch bites progressively: from the launch hold at rest to the wheels' speed at `clutch_lock_mps`.
            (coupled + (launch_rpm - coupled) * fade).max(spec.idle_rpm)
        } else if slipping {
            spec.idle_rpm
        } else {
            coupled
        };
        // A free or slipping engine spins up at a finite rate; a coupled one follows the wheels.
        self.rpm = if slipping || ratio == 0.0 {
            let rate = 20_000.0 * dt; // rpm per step: about 0.4 s from idle to the limiter
            self.rpm + (target_rpm - self.rpm).clamp(-rate, rate)
        } else {
            target_rpm
        };
        self.rpm = self.rpm.clamp(spec.idle_rpm * 0.8, spec.limiter_rpm + 200.0);

        let limiter = self.rpm >= spec.limiter_rpm;
        let friction = spec.friction_nm * self.rpm / spec.redline_rpm;
        let engine_torque = if limiter { -friction } else { spec.torque_at(self.rpm) * throttle - friction * (1.0 - throttle) };
        let shifting = self.shift_left > 0.0;
        self.shift_left = (self.shift_left - dt).max(0.0);

        // Aero and rolling, from the road speed.
        let v = speed_mps.abs();
        let drag = 0.5 * spec.air_density * spec.drag_area_m2 * v * v;
        let downforce = 0.5 * spec.air_density * spec.downforce_area_m2 * v * v;
        let rolling = if stopped { 0.0 } else { spec.rolling_resistance * (spec.mass_kg * G + downforce) };

        // Drive force at the wheels, signed along the car's forward axis. Engine braking only acts coupled.
        let mut drive = 0.0;
        if ratio > 0.0 && !shifting && (engine_torque > 0.0 || !slipping) {
            let direction = if self.gear < 0 { -1.0 } else { 1.0 };
            drive = engine_torque * ratio * spec.final_drive * spec.efficiency / spec.driven_radius_m() * direction;
        }
        let axle_torque_nm = drive * spec.driven_radius_m();
        let drive_ratio = ratio * spec.final_drive * if self.gear < 0 { -1.0 } else { 1.0 };
        let traction = self.traction_limit(downforce);
        let wheelspin = drive.abs() > traction;
        let traction_excess = (drive.abs() / traction.max(1.0) - 1.0).max(0.0);
        drive = drive.clamp(-traction, traction);

        // The engine's rotating mass, seen at the wheels, resists changes of speed: scale the net force by m / (m + m_rot).
        let resist = (drag + rolling) * speed_mps.signum();
        let m_rot = spec.inertia_kgm2 * (ratio * spec.final_drive / spec.driven_radius_m()).powi(2) * if slipping { 0.0 } else { 1.0 };
        let drive = resist + (drive - resist) * spec.mass_kg / (spec.mass_kg + m_rot);

        // Brakes, capped by the grip of all four tyres.
        let brake_total = (brake * spec.max_decel_g * spec.mass_kg * G).min(spec.grip_mu * (spec.mass_kg * G + downforce));
        let (brake_front, brake_rear) = (brake_total * spec.front_brake_bias, brake_total * (1.0 - spec.front_brake_bias));
        // Mechanical rear brake demand; the vehicle solver caps a locked tire at its actual loaded grip.
        let handbrake_rear = handbrake * spec.max_decel_g * spec.mass_kg * G;

        if self.auto && self.gear > 0 && !shifting {
            self.auto_shift(throttle, speed_mps);
        }

        let steer_angle = controls.steer.clamp(-1.0, 1.0) * spec.max_steer_rad / (1.0 + v / spec.steer_speed_mps);

        // Temperatures follow the engine's load (the share of peak power it is making) with a lag; oil runs hotter
        // than coolant and heats most under sustained high revs. Oil pressure rises with rpm and thins as oil heats.
        // Boost (a turbocharged spec) builds with throttle and revs, with lag.
        let (peak_kw, _) = spec.peak_power();
        let load = (engine_torque.max(0.0) * self.rpm * std::f32::consts::TAU / 60_000.0 / peak_kw.max(1.0)).clamp(0.0, 1.0);
        let rev = (self.rpm / spec.redline_rpm).clamp(0.0, 1.1);
        let water_target = spec.water_cruise_c + spec.water_load_c * load;
        let oil_target = water_target + spec.oil_above_water_c + spec.oil_load_c * load * rev;
        self.water_c += (water_target - self.water_c) * (1.0 - (-dt / spec.water_tau_s).exp());
        self.oil_c += (oil_target - self.oil_c) * (1.0 - (-dt / spec.oil_tau_s).exp());
        let oil_pressure = (1.2 + 4.3 * rev) * (1.0 - ((self.oil_c - 90.0).max(0.0) * 0.006)).clamp(0.6, 1.0);
        let boost_target = if spec.boost_bar > 0.0 { spec.boost_bar * throttle * ((self.rpm - 2000.0) / 2500.0).clamp(0.0, 1.0) } else { 0.0 };
        let boost_target = if throttle < 0.05 && spec.boost_bar > 0.0 { -0.6 } else { boost_target };
        self.boost += (boost_target - self.boost) * (1.0 - (-dt / spec.boost_lag_s).exp());
        self.last = Output {
            drive_force_n: drive,
            axle_torque_nm, drive_ratio,
            brake_front_n: brake_front,
            brake_rear_n: brake_rear,
            handbrake_rear_n: handbrake_rear,
            throttle, brake, handbrake,
            drag_n: drag,
            rolling_n: rolling,
            downforce_n: downforce,
            steer_angle,
            rpm: self.rpm,
            gear: self.gear,
            engine_torque_nm: engine_torque,
            power_kw: engine_torque.max(0.0) * self.rpm * std::f32::consts::TAU / 60_000.0,
            speed_mps,
            shifting,
            limiter,
            wheelspin,
            traction_excess,
            launch,
            water_temp_c: self.water_c,
            oil_temp_c: self.oil_c,
            oil_pressure_bar: oil_pressure,
            boost_bar: if spec.boost_bar > 0.0 { self.boost } else { 0.0 },
        };
        self.last
    }

    /// The most force the driven axle can put down: µ × its load, with the load transfer the force itself causes
    /// (F·h/L onto the rear, off the front) and its share of the downforce.
    fn traction_limit(&self, downforce: f32) -> f32 {
        let s = &self.spec;
        let weight = s.mass_kg * G;
        let transfer = s.cog_height_m / s.wheelbase_m;
        match s.drive {
            // F ≤ µ (N + F·h/L)  →  F ≤ µN / (1 − µh/L)
            Drive::Rwd => {
                let n = weight * (1.0 - s.weight_front) + downforce * (1.0 - s.weight_front);
                s.grip_mu * n / (1.0 - s.grip_mu * transfer).max(0.2)
            }
            // F ≤ µ (N − F·h/L)  →  F ≤ µN / (1 + µh/L)
            Drive::Fwd => {
                let n = weight * s.weight_front + downforce * s.weight_front;
                s.grip_mu * n / (1.0 + s.grip_mu * transfer)
            }
            Drive::Awd => s.grip_mu * (weight + downforce),
        }
    }

    fn auto_shift(&mut self, throttle: f32, speed_mps: f32) {
        let s = &self.spec;
        let top = s.ratios.len() as i32;
        if self.rpm >= s.upshift_rpm && self.gear < top {
            self.begin_shift(self.gear + 1);
            return;
        }
        if self.gear > 1 {
            let lower = self.rpm_in(self.gear - 1, speed_mps);
            // Downshift when the engine bogs, or on kickdown when the lower gear has room to rev.
            let bogging = self.rpm < s.downshift_rpm && lower < s.upshift_rpm - 500.0;
            let kickdown = throttle > 0.9 && self.rpm < 6000.0 && lower < s.upshift_rpm - 1200.0;
            if bogging || kickdown {
                self.begin_shift(self.gear - 1);
            }
        }
    }

    fn begin_shift(&mut self, gear: i32) {
        self.gear = gear;
        self.shift_left = self.spec.shift_time_s;
    }

    /// The gear's overall ratio without the final drive; 0 in neutral.
    fn ratio(&self) -> f32 {
        match self.gear {
            g if g < 0 => self.spec.reverse_ratio,
            0 => 0.0,
            g => self.spec.ratios[(g - 1) as usize],
        }
    }

    /// Engine speed in `gear` at `speed_mps`.
    fn rpm_in(&self, gear: i32, speed_mps: f32) -> f32 {
        let s = &self.spec;
        let ratio = if gear < 0 { s.reverse_ratio } else { s.ratios[(gear.max(1) - 1) as usize] };
        speed_mps.abs() / s.driven_radius_m() * 60.0 / std::f32::consts::TAU * ratio * s.final_drive
    }
}

/// One straight-line run at full throttle from rest, as a point mass on a flat road: (time s, speed m/s, output)
/// every `dt`, until `seconds`. Uses the same `Car::step`, so it checks the driveline against published figures.
pub fn straight_line(spec: &CarSpec, seconds: f32, dt: f32) -> Vec<(f32, f32, Output)> {
    let mut car = Car::new(spec.clone());
    let (mut t, mut v) = (0.0f32, 0.0f32);
    let mut samples = Vec::new();
    while t < seconds {
        let out = car.step(Controls { throttle: 1.0, ..Default::default() }, v, dt);
        let net = out.drive_force_n - (out.drag_n + out.rolling_n) * v.signum();
        v = (v + net / spec.mass_kg * dt).max(0.0);
        t += dt;
        samples.push((t, v, out));
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_keys_brake_to_rest_before_reversing_and_both_keys_only_brake() {
        let mut car=Car::new(CarSpec::porsche_992_gt3_rs());
        let s=Controls{brake:1.0,..Default::default()};
        for speed in [15.0,0.2,0.002] {
            let out=car.step(s,speed,1.0/60.0);
            assert!(out.gear>0 && out.brake>0.9 && out.throttle==0.0,"cannot reverse while rolling: {out:?}");
        }
        let out=car.step(s,0.0,1.0/60.0);assert_eq!(out.gear,-1);
        for _ in 0..10 {car.step(s,0.0,1.0/60.0);}
        assert!(car.last().drive_force_n<0.0 && car.last().throttle>0.9 && car.last().brake==0.0);
        let w=Controls{throttle:1.0,..Default::default()};
        for speed in [-15.0,-0.2,-0.002] {
            let out=car.step(w,speed,1.0/60.0);
            assert!(out.gear<0 && out.brake>0.9 && out.throttle==0.0,"cannot drive forward before stopping: {out:?}");
        }
        assert_eq!(car.step(w,0.0,1.0/60.0).gear,1);
        let both=car.step(Controls{throttle:1.0,brake:1.0,..Default::default()},0.0,1.0/60.0);
        assert_eq!(both.gear,1);assert_eq!(both.throttle,0.0);assert_eq!(both.brake,1.0);
        // Gear alone cannot turn W into reverse gas when the car is rolling backwards downhill.
        let rollback=car.step(w,-2.0,1.0/60.0);assert_eq!(rollback.throttle,0.0);assert_eq!(rollback.brake,1.0);
    }

    #[test]
    fn handbrake_is_independent_of_service_brakes_and_direction() {
        let mut car=Car::new(CarSpec::porsche_992_gt3_rs());
        let held=car.step(Controls{handbrake:1.0,..Default::default()},12.0,1.0/60.0);
        assert!(held.handbrake_rear_n>0.0);assert_eq!(held.brake_front_n,0.0);assert_eq!(held.brake_rear_n,0.0);
        assert_eq!(held.brake,0.0);assert_eq!(held.handbrake,1.0);assert!(held.gear>0);
        let released=car.step(Controls::default(),12.0,1.0/60.0);
        assert_eq!(released.handbrake_rear_n,0.0);assert_eq!(released.handbrake,0.0);
    }

    fn run() -> Vec<(f32, f32, Output)> {
        straight_line(&CarSpec::porsche_992_gt3_rs(), 120.0, 1.0 / 120.0)
    }

    fn time_to(samples: &[(f32, f32, Output)], kmh: f32) -> f32 {
        samples.iter().find(|s| s.1 * 3.6 >= kmh).map_or(f32::INFINITY, |s| s.0)
    }

    #[test]
    fn zero_to_hundred_is_close_to_the_published_three_point_two_seconds() {
        let t = time_to(&run(), 100.0);
        assert!((2.9..=3.6).contains(&t), "0-100 km/h in {t:.2} s");
    }

    #[test]
    fn top_speed_is_close_to_the_published_296_kmh() {
        let samples = run();
        let top = samples.iter().map(|s| s.1 * 3.6).fold(0.0, f32::max);
        assert!((285.0..=305.0).contains(&top), "top speed {top:.1} km/h");
        // Drag-limited near peak power, in sixth or seventh depending on where the drag curve meets the power curve.
        let last = samples.last().unwrap().2;
        assert!(last.gear >= 6, "top speed in gear {}", last.gear);
    }

    #[test]
    fn it_shifts_up_through_the_gears_near_the_limiter() {
        let samples = run();
        let mut gears = vec![1];
        let mut rpm_before_shift = Vec::new();
        for pair in samples.windows(2) {
            if pair[1].2.gear != pair[0].2.gear {
                gears.push(pair[1].2.gear);
                rpm_before_shift.push(pair[0].2.rpm);
            }
        }
        assert_eq!(gears[..6], [1, 2, 3, 4, 5, 6]);
        assert!(rpm_before_shift.iter().all(|&r| r >= 8700.0), "upshifts at {rpm_before_shift:?}");
    }

    #[test]
    fn launch_holds_revs_and_spins_the_tyres_at_first() {
        let mut car = Car::new(CarSpec::porsche_992_gt3_rs());
        let mut out = Output::default();
        for _ in 0..12 {
            out = car.step(Controls { throttle: 1.0, ..Default::default() }, 0.0, 1.0 / 60.0);
        }
        assert!(out.launch && out.rpm > 4000.0, "launch rpm {}", out.rpm);
        assert!(out.wheelspin, "full launch torque exceeds the rear tyres' grip");
    }

    #[test]
    fn braking_at_a_standstill_selects_reverse_and_the_brake_then_drives_backwards() {
        let mut car = Car::new(CarSpec::porsche_992_gt3_rs());
        let mut out = car.step(Controls { brake: 1.0, ..Default::default() }, 0.0, 1.0 / 60.0);
        for _ in 0..20 {
            out = car.step(Controls { brake: 1.0, ..Default::default() }, 0.0, 1.0 / 60.0);
        }
        assert_eq!(out.gear, -1);
        assert!(out.drive_force_n < 0.0, "reverse drives backwards: {}", out.drive_force_n);
        let out = car.step(Controls { throttle: 1.0, ..Default::default() }, 0.0, 1.0 / 60.0);
        assert_eq!(out.gear, 1, "throttle at a standstill selects first");
    }

    #[test]
    fn a_closed_throttle_engine_brakes_and_the_brakes_scale_with_the_pedal() {
        let mut car = Car::new(CarSpec::porsche_992_gt3_rs());
        let coast = car.step(Controls::default(), 30.0, 1.0 / 60.0);
        assert!(coast.drive_force_n < 0.0, "engine braking: {}", coast.drive_force_n);
        // Below the grip cap, brake force follows the pedal.
        let light = car.step(Controls { brake: 0.3, ..Default::default() }, 30.0, 1.0 / 60.0);
        let firm = car.step(Controls { brake: 0.6, ..Default::default() }, 30.0, 1.0 / 60.0);
        let total = |o: &Output| o.brake_front_n + o.brake_rear_n;
        assert!((total(&firm) / total(&light) - 2.0).abs() < 0.01);
        assert!(firm.brake_front_n > firm.brake_rear_n, "front bias");
        // At full pedal the tyres, not the brakes, set the limit.
        let full = car.step(Controls { brake: 1.0, ..Default::default() }, 30.0, 1.0 / 60.0);
        let spec = CarSpec::porsche_992_gt3_rs();
        assert!(total(&full) <= spec.grip_mu * (spec.mass_kg * G + full.downforce_n) + 1.0);
    }

    #[test]
    fn hard_running_heats_the_oil_more_than_the_water_and_revs_raise_oil_pressure() {
        let mut car = Car::new(CarSpec::porsche_992_gt3_rs());
        let idle = car.step(Controls::default(), 0.0, 1.0 / 60.0);
        let mut out = idle;
        // Two minutes flat out in third at about 8,000 rpm.
        for _ in 0..(120 * 60) {
            out = car.step(Controls { throttle: 1.0, ..Default::default() }, 45.0, 1.0 / 60.0);
        }
        assert!(out.oil_temp_c > idle.oil_temp_c + 10.0, "oil {} -> {}", idle.oil_temp_c, out.oil_temp_c);
        assert!(out.water_temp_c > idle.water_temp_c && out.water_temp_c < 105.0, "water {}", out.water_temp_c);
        assert!(out.oil_temp_c > out.water_temp_c, "oil runs hotter than water");
        assert!(out.oil_pressure_bar > idle.oil_pressure_bar + 2.0, "oil pressure {} -> {}", idle.oil_pressure_bar, out.oil_pressure_bar);
        assert_eq!(out.boost_bar, 0.0, "the GT3 RS has no turbo");
    }

    #[test]
    fn paddles_shift_and_refuse_to_over_rev() {
        let mut car = Car::new(CarSpec::porsche_992_gt3_rs());
        car.auto = false;
        let out = car.step(Controls { shift: 1, ..Default::default() }, 20.0, 1.0 / 60.0);
        assert_eq!(out.gear, 2);
        // 2nd → 1st at 40 m/s would put the engine far past the limiter.
        let out = car.step(Controls { shift: -1, ..Default::default() }, 40.0, 1.0 / 60.0);
        assert_eq!(out.gear, 2);
    }
}
