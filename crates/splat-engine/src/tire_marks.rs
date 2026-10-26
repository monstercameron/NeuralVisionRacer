//! Distance-sampled trails and bounded stationary rubber patches from actual wheel/road slip.
use rapier3d::prelude::Vector;
use splat_render::tire::{TireMark, CAPACITY, LIFETIME};
use std::collections::VecDeque;

const SPACING: f32 = 0.25;

#[derive(Clone, Copy, Debug, Default)]
pub struct TireContact {
    pub point: Vector,
    pub normal: Vector,
    pub forward: Vector,
    pub radius: f32,
    pub width: f32,
    pub angular_speed: f32,
    pub longitudinal_speed: f32,
    pub lateral_speed: f32,
    /// Applied normal load / this wheel's static load.
    pub load: f32,
    pub grip_loss: f32,
    /// Requested drive beyond the mechanics' traction limit, only on a wheel receiving drive.
    pub drive_excess: f32,
    pub rpm: f32,
    pub locked: bool,
}

impl TireContact {
    fn appearance(self) -> (f32, f32) {
        let rubbing = (self.lateral_speed.abs() - 0.25).max(0.0) / 4.0;
        let locked_slide = if self.locked { (self.longitudinal_speed.abs() - 0.25).max(0.0) / 4.0 } else { 0.0 };
        let longitudinal_slip = (self.angular_speed * self.radius - self.longitudinal_speed).abs();
        let slip = (rubbing + longitudinal_slip / 4.0 + self.grip_loss * 1.3 + self.drive_excess + locked_slide).clamp(0.0, 1.0);
        let motion = (self.longitudinal_speed.hypot(self.lateral_speed).max(longitudinal_slip) / 1.5).clamp(0.0, 1.0);
        // A faint rolling tread makes the path legible; rubber transfer dominates with lost grip.
        let strength = (0.14 + slip * 0.58) * self.load.clamp(0.0, 1.5).sqrt().min(1.15) * motion;
        (strength.min(0.8), slip)
    }
    fn valid(self) -> bool {
        self.point.is_finite()
            && self.normal.is_finite()
            && self.forward.is_finite()
            && self.normal.y > 0.35
            && self.load > 0.025
            && self.radius > 0.0
            && self.angular_speed.is_finite()
            && self.longitudinal_speed.is_finite()
            && self.lateral_speed.is_finite()
    }
}

#[derive(Clone, Copy)]
struct Anchor {
    contact: TireContact,
    phase: f64,
    strength: f32,
    smear: f32,
}

#[derive(Clone, Copy)]
struct Patch { id: u64, point: Vector, normal: Vector, strength: f32 }

#[derive(Default)]
pub struct TireTrails {
    pub marks: VecDeque<TireMark>,
    pub revision: u64,
    pub time: f64,
    anchors: [Option<Anchor>; 4],
    previous: [Option<Vector>; 4],
    phase: [f64; 4],
    patches: [Option<Patch>; 4],
    emitted: u64,
    evicted: u64,
    breaks: u64,
    latest: [Option<TireContact>; 4],
    clock: Option<fn() -> f64>,
    metrics: splat_render::metrics::Metrics,
}

impl TireTrails {
    pub fn set_clock(&mut self, clock: fn() -> f64) {
        self.clock = Some(clock);
    }
    pub fn break_tracks(&mut self) {
        self.anchors = [None; 4];
        self.previous = [None; 4];
        self.latest = [None; 4];
        self.patches = [None; 4];
        self.breaks += 1;
    }
    pub fn clear(&mut self) {
        self.marks.clear();
        self.revision += 1;
        self.break_tracks();
    }
    pub fn step(&mut self, contacts: [Option<TireContact>; 4], dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let started = self.clock.map(|c| c());
        self.time += f64::from(dt);
        while self
            .marks
            .front()
            .is_some_and(|m| self.time - f64::from(m.born) > f64::from(LIFETIME))
        {
            self.marks.pop_front();
            self.evicted += 1;
            self.revision += 1;
        }
        self.latest = contacts;
        for (i, contact) in contacts.into_iter().enumerate() {
            let Some(c) = contact.filter(|c| c.valid()) else {
                if self.anchors[i].is_some() {
                    self.breaks += 1;
                }
                self.anchors[i] = None;
                self.previous[i] = None;
                self.patches[i] = None;
                continue;
            };
            if self.previous[i].is_some() {
                self.phase[i] += f64::from(c.angular_speed * c.radius * dt);
            }
            let (strength, smear) = c.appearance();
            let now = Anchor {
                contact: c,
                phase: self.phase[i],
                strength,
                smear,
            };
            let slip_speed = (c.angular_speed * c.radius - c.longitudinal_speed).abs();
            if c.longitudinal_speed.hypot(c.lateral_speed) < 0.6 && slip_speed > 0.75 {
                self.deposit_stationary(i, c, slip_speed, dt);
            } else { self.patches[i] = None; }
            let jumped = self.previous[i].is_some_and(|p| (c.point - p).length() > 3.0);
            self.previous[i] = Some(c.point);
            let Some(mut anchor) =
                self.anchors[i].filter(|a| !jumped && a.contact.normal.dot(c.normal) > 0.65)
            else {
                self.anchors[i] = Some(now);
                continue;
            };
            let distance = (c.point - anchor.contact.point).length();
            // Rolling trails remain distance sampled; stationary slip accumulates in one patch above.
            if distance < SPACING {
                continue;
            }
            let pieces = (distance / SPACING).floor().min(64.0) as usize;
            for _ in 0..pieces {
                let remaining = (c.point - anchor.contact.point).length();
                if remaining < SPACING - 1e-4 {
                    break;
                }
                let t = (SPACING / remaining).min(1.0);
                let end = anchor.contact.point.lerp(c.point, t);
                let end_phase = anchor.phase + (now.phase - anchor.phase) * f64::from(t);
                let alpha = anchor.strength + (strength - anchor.strength) * t;
                let blur = anchor.smear + (smear - anchor.smear) * t;
                if alpha.max(anchor.strength) > 0.005 {
                    let normal = (anchor.contact.normal + c.normal).normalize_or_zero();
                    let forward = (anchor.contact.forward + c.forward).normalize_or_zero();
                    self.push(TireMark {
                        start: anchor.contact.point.to_array(),
                        end: end.to_array(),
                        normal: normal.to_array(),
                        forward: forward.to_array(),
                        width: c.width,
                        phase_start: anchor.phase.rem_euclid(10.0) as f32,
                        phase_end: (anchor.phase.rem_euclid(10.0) + end_phase - anchor.phase)
                            as f32,
                        strength: (anchor.strength + alpha) * 0.5,
                        smear: (anchor.smear + blur) * 0.5,
                        born: self.time as f32,
                        last_deposit: self.time as f32,
                    });
                }
                anchor = Anchor {
                    contact: TireContact { point: end, ..c },
                    phase: end_phase,
                    strength: alpha,
                    smear: blur,
                };
            }
            self.anchors[i] = Some(anchor);
        }
        if let (Some(clock), Some(started)) = (self.clock, started) {
            self.metrics.record("generate", clock() - started);
        }
    }
    fn deposit_stationary(&mut self, i: usize, c: TireContact, slip: f32, dt: f32) {
        let base = self.emitted - self.marks.len() as u64;
        let patch = self.patches[i].filter(|p|
            (c.point - p.point).length() < 0.12 && c.normal.dot(p.normal) > 0.9);
        let existing = patch.filter(|p| p.id >= base)
            .and_then(|p| self.marks.get_mut((p.id - base) as usize));
        // Exponential rubber transfer depends on sliding distance and load, not frame count.
        let transfer = 1.0 - (-0.16 * slip * c.load.min(1.5) * dt).exp();
        if let Some(mark) = existing {
            mark.strength += (0.8 - mark.strength) * transfer;
            mark.last_deposit = self.time as f32;
            if let Some(patch) = self.patches[i].as_mut() { patch.strength = mark.strength; }
            self.revision += 1;
        } else {
            let id = self.emitted;
            // Recycle a long-running patch at history expiry without flashing its opacity.
            let previous = patch.map_or(0.0, |p| p.strength);
            let strength = previous + (0.8 - previous) * transfer;
            let half = c.forward.normalize_or_zero() * 0.125;
            self.push(TireMark {
                start: (c.point - half).to_array(), end: (c.point + half).to_array(),
                normal: c.normal.to_array(), forward: c.forward.to_array(), width: c.width,
                phase_start: self.phase[i].rem_euclid(10.0) as f32,
                phase_end: self.phase[i].rem_euclid(10.0) as f32 + 0.25,
                strength, smear: 1.0,
                born: self.time as f32, last_deposit: self.time as f32,
            });
            self.patches[i] = Some(Patch { id, point: c.point, normal: c.normal, strength });
        }
    }
    fn push(&mut self, mark: TireMark) {
        if self.marks.len() == CAPACITY {
            self.marks.pop_front();
            self.evicted += 1;
        }
        self.marks.push_back(mark);
        self.emitted += 1;
        self.revision += 1;
    }
    pub fn profile_json(&self) -> serde_json::Value {
        serde_json::json!({"active":self.marks.len(),"capacity":CAPACITY,"emitted":self.emitted,"cpu":self.metrics.json(),
            "evicted":self.evicted,"continuity_breaks":self.breaks,"simulation_seconds":self.time,
            "traction_source":"wheel surface minus road velocity, lateral motion and impulse saturation",
            "wheel_speed_source":"independent driven rotation from axle torque, loaded grip and engine/gearing; passive wheels roll",
            "wheels":self.latest.map(|c| c.map(|c| {let (strength,smear)=c.appearance();serde_json::json!({
                "point":c.point.to_array(),"load":c.load,"road_mps":c.longitudinal_speed,
                "side_mps":c.lateral_speed,"longitudinal_slip_mps":c.angular_speed*c.radius-c.longitudinal_speed,"wheel_rad_s":c.angular_speed,"wheel_surface_mps":c.angular_speed*c.radius,
                "grip_loss":c.grip_loss,"drive_excess":c.drive_excess,"rpm":c.rpm,"locked":c.locked,"strength":strength,"smear":smear})}))})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contact(z: f32, speed: f32) -> TireContact {
        TireContact {
            point: Vector::new(0.0, 0.0, z),
            normal: Vector::Y,
            forward: Vector::Z,
            radius: 0.35,
            width: 0.275,
            angular_speed: speed / 0.35,
            longitudinal_speed: speed,
            load: 1.0,
            ..Default::default()
        }
    }
    #[test]
    fn rest_revs_and_airborne_do_not_paint() {
        let mut trails = TireTrails::default();
        for i in 0..200 {
            let mut c = contact(0.0, 0.0);
            c.rpm = 9000.0;
            c.drive_excess = 1.0;
            trails.step([Some(c), None, None, None], 1.0 / 60.0);
            if i > 100 {
                trails.step([None; 4], 1.0 / 60.0);
            }
        }
        assert!(trails.marks.is_empty());
    }
    #[test]
    fn stationary_transfer_is_bounded_substep_independent_and_requires_slip() {
        let run = |hz: usize| {
            let mut trails = TireTrails::default();
            let c = TireContact { angular_speed: 50.0, ..contact(0.0, 0.0) };
            for _ in 0..hz * 5 { trails.step([Some(c), None, None, None], 1.0 / hz as f32); }
            assert_eq!(trails.marks.len(), 1);
            trails
        };
        let a = run(60); let b = run(180);
        assert!((a.marks[0].strength - b.marks[0].strength).abs() < 0.0001);
        assert!(a.marks[0].strength > 0.79 && a.marks[0].strength <= 0.8);
        let mut t = run(60);
        for _ in 0..60 * 121 { t.step([Some(TireContact { angular_speed: 50.0, ..contact(0.0, 0.0) }), None, None, None], 1.0 / 60.0); }
        assert_eq!(t.marks.len(), 1);
        assert!(t.marks[0].strength > 0.79, "continuous burnout survives bounded-history recycling");
        let before = t.marks[0].strength;
        for _ in 0..60 { t.step([Some(contact(0.0, 0.0)), None, None, None], 1.0 / 60.0); }
        assert_eq!(t.marks[0].strength, before);
        t.step([None; 4], LIFETIME + 1.0);
        assert!(t.marks.is_empty());
        t.step([Some(TireContact { angular_speed: -50.0, ..contact(0.0, 0.0) }), None, None, None], 1.0 / 60.0);
        assert_eq!(t.marks.len(), 1, "expired patch handles cannot alias another wheel");
    }

    #[test]
    fn distance_sampling_is_substep_independent_and_breaks_on_jump() {
        let run = |hz: u32| {
            let mut t = TireTrails::default();
            for i in 0..=hz * 2 {
                t.step(
                    [
                        Some(contact(i as f32 * 10.0 / hz as f32, 10.0)),
                        None,
                        None,
                        None,
                    ],
                    1.0 / hz as f32,
                );
            }
            t
        };
        let a = run(60);
        let mut b = run(180);
        assert!(a.marks.len().abs_diff(b.marks.len()) <= 1);
        for (a, b) in a.marks.iter().zip(&b.marks) {
            assert!((a.end[2] - b.end[2]).abs() < 0.001);
            assert!((a.phase_end - b.phase_end).abs() < 0.003);
        }
        let before = b.marks.len();
        b.step([Some(contact(500.0, 10.0)), None, None, None], 1.0 / 60.0);
        assert_eq!(before, b.marks.len());
        b.step([None; 4], 1.0 / 60.0);
        b.step([Some(contact(501.0, 10.0)), None, None, None], 1.0 / 60.0);
        assert_eq!(before, b.marks.len());
    }
    #[test]
    fn grip_loss_and_slide_strengthen_and_smear_marks_not_rpm_alone() {
        let c = contact(0.0, 15.0);
        let rolling = c.appearance();
        assert_eq!(rolling, TireContact { rpm: 9000.0, ..c }.appearance());
        let sliding = TireContact {
            lateral_speed: 5.0,
            grip_loss: 0.5,
            ..c
        }
        .appearance();
        let launch = TireContact {
            drive_excess: 0.7,
            ..c
        }
        .appearance();
        assert!(sliding.0 > rolling.0 * 3.0 && sliding.1 > rolling.1);
        assert!(launch.0 > rolling.0 * 3.0);
    }
    #[test]
    fn history_is_bounded_expires_and_clears() {
        let mut t = TireTrails::default();
        let m = TireMark {
            start: [0.0; 3],
            end: [0.0, 0.0, 1.0],
            normal: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, 1.0],
            width: 0.3,
            phase_start: 0.0,
            phase_end: 1.0,
            strength: 0.5,
            smear: 0.0,
            born: 0.0,
            last_deposit: 0.0,
        };
        for _ in 0..CAPACITY + 10 {
            t.push(m);
        }
        assert_eq!(t.marks.len(), CAPACITY);
        assert_eq!(t.evicted, 10);
        t.step([None; 4], LIFETIME + 1.0);
        assert!(t.marks.is_empty());
        t.push(m);
        t.clear();
        assert!(t.marks.is_empty());
    }
}
