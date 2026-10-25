//! A car's specification, read from a `.car` file: `[section]` headers, `key = value` lines, `#` comments.
//!
//! Every key the mechanics use is required, so a typo fails loudly with its line number rather than silently using
//! a default. Unknown keys are errors too. Descriptive keys (`name`, `type`, `layout`, tyre sizes) are kept as text.

use std::collections::BTreeMap;

/// Where the engine sits. It sets nothing in the mechanics yet beyond what `weight_front` already says, but the
/// layout is part of the spec, and the console and telemetry report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    FrontEngine,
    MidEngine,
    RearEngine,
}

/// Which axle the engine drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    Rwd,
    Fwd,
    Awd,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CarSpec {
    pub name: String,
    pub mass_kg: f32,
    pub layout: Layout,
    pub drive: Drive,
    /// Share of the static weight on the front axle.
    pub weight_front: f32,
    pub cog_height_m: f32,
    pub length_m: f32,
    pub wheelbase_m: f32,
    pub track_front_m: f32,
    pub track_rear_m: f32,
    /// Nose to front axle.
    pub front_overhang_m: f32,

    pub engine_type: String,
    pub idle_rpm: f32,
    pub redline_rpm: f32,
    pub limiter_rpm: f32,
    /// (rpm, Nm), ascending rpm.
    pub torque_curve: Vec<(f32, f32)>,
    /// Engine braking torque at redline with the throttle closed; scales linearly with rpm.
    pub friction_nm: f32,
    pub inertia_kgm2: f32,
    pub launch_rpm: f32,
    /// Peak boost in bar of gauge pressure; 0 for a naturally aspirated engine. Optional in the file.
    pub boost_bar: f32,

    pub gearbox_type: String,
    /// Forward ratios, first gear first.
    pub ratios: Vec<f32>,
    pub reverse_ratio: f32,
    pub final_drive: f32,
    pub shift_time_s: f32,
    pub efficiency: f32,
    pub upshift_rpm: f32,
    pub downshift_rpm: f32,

    pub tyre_front: String,
    pub tyre_rear: String,
    pub front_radius_m: f32,
    pub rear_radius_m: f32,
    pub grip_mu: f32,
    pub rolling_resistance: f32,

    /// Drag coefficient × frontal area.
    pub drag_area_m2: f32,
    /// Lift coefficient × area, positive for downforce.
    pub downforce_area_m2: f32,
    pub air_density: f32,

    /// Front wheel angle at full lock, standing still.
    pub max_steer_rad: f32,
    /// The lock halves at this speed.
    pub steer_speed_mps: f32,

    pub max_decel_g: f32,
    pub front_brake_bias: f32,

    /// Launch control fades the clutch slip out by this speed (m/s). Optional (`engine.clutch_lock_kmh`).
    pub clutch_lock_mps: f32,
    /// Cooling, all optional in `[cooling]`: the coolant temperature at cruise and what full load adds; how much
    /// hotter the oil runs and what load at high revs adds; the time constants; the warm start; the turbo's lag.
    pub water_cruise_c: f32,
    pub water_load_c: f32,
    pub oil_above_water_c: f32,
    pub oil_load_c: f32,
    pub water_tau_s: f32,
    pub oil_tau_s: f32,
    pub start_water_c: f32,
    pub start_oil_c: f32,
    pub boost_lag_s: f32,
}

/// The Porsche 911 GT3 RS (992), the default car.
pub const PORSCHE_992_GT3_RS: &str = include_str!("../specs/porsche-992-gt3rs.car");

impl CarSpec {
    /// The built-in 992 GT3 RS.
    pub fn porsche_992_gt3_rs() -> Self {
        Self::parse(PORSCHE_992_GT3_RS).expect("the built-in spec parses")
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut values: BTreeMap<String, (usize, String)> = BTreeMap::new();
        let mut section = String::new();
        for (index, raw) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.trim().to_string();
                continue;
            }
            let (key, value) = line.split_once('=').ok_or(format!("line {line_no}: expected `key = value`"))?;
            values.insert(format!("{section}.{}", key.trim()), (line_no, value.trim().to_string()));
        }
        let mut reader = Reader { values, used: Vec::new() };
        let spec = Self {
            name: reader.text("car.name")?,
            mass_kg: reader.positive("car.mass_kg")?,
            layout: match reader.text("car.layout")?.as_str() {
                "front-engine" => Layout::FrontEngine,
                "mid-engine" => Layout::MidEngine,
                "rear-engine" => Layout::RearEngine,
                other => return Err(format!("car.layout: `{other}` is not front-engine, mid-engine or rear-engine")),
            },
            drive: match reader.text("car.drive")?.as_str() {
                "rwd" => Drive::Rwd,
                "fwd" => Drive::Fwd,
                "awd" => Drive::Awd,
                other => return Err(format!("car.drive: `{other}` is not rwd, fwd or awd")),
            },
            weight_front: reader.number("car.weight_front")?,
            cog_height_m: reader.positive("car.cog_height_m")?,
            length_m: reader.positive("car.length_m")?,
            wheelbase_m: reader.positive("car.wheelbase_m")?,
            track_front_m: reader.positive("car.track_front_m")?,
            track_rear_m: reader.positive("car.track_rear_m")?,
            front_overhang_m: reader.number("car.front_overhang_m")?,

            engine_type: reader.text("engine.type")?,
            idle_rpm: reader.positive("engine.idle_rpm")?,
            redline_rpm: reader.positive("engine.redline_rpm")?,
            limiter_rpm: reader.positive("engine.limiter_rpm")?,
            torque_curve: reader.curve("engine.torque_curve")?,
            friction_nm: reader.number("engine.friction_nm")?,
            inertia_kgm2: reader.positive("engine.inertia_kgm2")?,
            launch_rpm: reader.positive("engine.launch_rpm")?,
            boost_bar: reader.optional_number("engine.boost_bar", 0.0)?,

            gearbox_type: reader.text("gearbox.type")?,
            ratios: reader.list("gearbox.ratios")?,
            reverse_ratio: reader.positive("gearbox.reverse")?,
            final_drive: reader.positive("gearbox.final_drive")?,
            shift_time_s: reader.number("gearbox.shift_time_s")?,
            efficiency: reader.positive("gearbox.efficiency")?,
            upshift_rpm: reader.positive("gearbox.upshift_rpm")?,
            downshift_rpm: reader.positive("gearbox.downshift_rpm")?,

            tyre_front: reader.text("tyres.front")?,
            tyre_rear: reader.text("tyres.rear")?,
            front_radius_m: reader.positive("tyres.front_radius_m")?,
            rear_radius_m: reader.positive("tyres.rear_radius_m")?,
            grip_mu: reader.positive("tyres.grip_mu")?,
            rolling_resistance: reader.number("tyres.rolling_resistance")?,

            drag_area_m2: reader.number("aero.drag_area_m2")?,
            downforce_area_m2: reader.number("aero.downforce_area_m2")?,
            air_density: reader.positive("aero.air_density")?,

            max_steer_rad: reader.positive("steering.max_angle_deg")?.to_radians(),
            steer_speed_mps: reader.positive("steering.speed_sensitivity_mps")?,

            max_decel_g: reader.positive("brakes.max_decel_g")?,
            front_brake_bias: reader.number("brakes.front_bias")?,

            clutch_lock_mps: reader.optional_number("engine.clutch_lock_kmh", 36.0)? / 3.6,
            water_cruise_c: reader.optional_number("cooling.water_cruise_c", 88.0)?,
            water_load_c: reader.optional_number("cooling.water_load_c", 10.0)?,
            oil_above_water_c: reader.optional_number("cooling.oil_above_water_c", 8.0)?,
            oil_load_c: reader.optional_number("cooling.oil_load_c", 22.0)?,
            water_tau_s: reader.optional_number("cooling.water_tau_s", 40.0)?.max(0.1),
            oil_tau_s: reader.optional_number("cooling.oil_tau_s", 90.0)?.max(0.1),
            start_water_c: reader.optional_number("cooling.start_water_c", 82.0)?,
            start_oil_c: reader.optional_number("cooling.start_oil_c", 85.0)?,
            boost_lag_s: reader.optional_number("cooling.boost_lag_s", 0.35)?.max(0.01),
        };
        reader.finish()?;
        spec.check()?;
        Ok(spec)
    }

    fn check(&self) -> Result<(), String> {
        if self.ratios.is_empty() {
            return Err("gearbox.ratios: at least one gear".into());
        }
        if self.ratios.windows(2).any(|w| w[1] >= w[0]) {
            return Err("gearbox.ratios: each gear must be taller (smaller) than the one before".into());
        }
        if self.torque_curve.len() < 2 || self.torque_curve.windows(2).any(|w| w[1].0 <= w[0].0) {
            return Err("engine.torque_curve: at least two points with rising rpm".into());
        }
        if !(0.0..=1.0).contains(&self.weight_front) || !(0.0..=1.0).contains(&self.front_brake_bias) {
            return Err("car.weight_front and brakes.front_bias are shares between 0 and 1".into());
        }
        if self.downshift_rpm >= self.upshift_rpm || self.upshift_rpm > self.limiter_rpm {
            return Err("gearbox: downshift_rpm < upshift_rpm <= engine.limiter_rpm".into());
        }
        Ok(())
    }

    /// Engine torque at full throttle, interpolated along the curve; held flat beyond its ends.
    pub fn torque_at(&self, rpm: f32) -> f32 {
        let curve = &self.torque_curve;
        if rpm <= curve[0].0 {
            return curve[0].1;
        }
        for pair in curve.windows(2) {
            let ((r0, t0), (r1, t1)) = (pair[0], pair[1]);
            if rpm <= r1 {
                return t0 + (t1 - t0) * (rpm - r0) / (r1 - r0);
            }
        }
        curve[curve.len() - 1].1
    }

    /// Full-throttle power in kW at `rpm`.
    pub fn power_kw_at(&self, rpm: f32) -> f32 {
        self.torque_at(rpm) * rpm * std::f32::consts::TAU / 60.0 / 1000.0
    }

    /// The peak of the power curve, sampled every 50 rpm: (kW, rpm).
    pub fn peak_power(&self) -> (f32, f32) {
        let mut best = (0.0, 0.0);
        let mut rpm = self.idle_rpm;
        while rpm <= self.limiter_rpm {
            let kw = self.power_kw_at(rpm);
            if kw > best.0 {
                best = (kw, rpm);
            }
            rpm += 50.0;
        }
        best
    }

    /// Radius of the driven wheels (the rear for rwd and awd).
    pub fn driven_radius_m(&self) -> f32 {
        match self.drive {
            Drive::Fwd => self.front_radius_m,
            Drive::Rwd | Drive::Awd => self.rear_radius_m,
        }
    }
}

struct Reader {
    values: BTreeMap<String, (usize, String)>,
    used: Vec<String>,
}

impl Reader {
    fn text(&mut self, key: &str) -> Result<String, String> {
        self.used.push(key.to_string());
        self.values.get(key).map(|(_, v)| v.clone()).ok_or(format!("missing `{key}`"))
    }

    fn number(&mut self, key: &str) -> Result<f32, String> {
        let line = self.values.get(key).map_or(0, |(l, _)| *l);
        self.text(key)?.parse().map_err(|_| format!("line {line}: `{key}` must be a number"))
    }

    /// A number that may be left out of the file.
    fn optional_number(&mut self, key: &str, default: f32) -> Result<f32, String> {
        if self.values.contains_key(key) { self.number(key) } else { Ok(default) }
    }

    fn positive(&mut self, key: &str) -> Result<f32, String> {
        let value = self.number(key)?;
        if value > 0.0 { Ok(value) } else { Err(format!("`{key}` must be positive")) }
    }

    fn list(&mut self, key: &str) -> Result<Vec<f32>, String> {
        self.text(key)?
            .split_whitespace()
            .map(|v| v.parse().map_err(|_| format!("`{key}`: `{v}` is not a number")))
            .collect()
    }

    fn curve(&mut self, key: &str) -> Result<Vec<(f32, f32)>, String> {
        self.text(key)?
            .split_whitespace()
            .map(|pair| {
                let (rpm, nm) = pair.split_once(':').ok_or(format!("`{key}`: `{pair}` is not rpm:Nm"))?;
                Ok((rpm.parse().map_err(|_| format!("`{key}`: bad rpm `{rpm}`"))?, nm.parse().map_err(|_| format!("`{key}`: bad torque `{nm}`"))?))
            })
            .collect()
    }

    fn finish(self) -> Result<(), String> {
        let unknown: Vec<String> = self
            .values
            .iter()
            .filter(|(k, _)| !self.used.contains(k))
            .map(|(k, (line, _))| format!("line {line}: unknown key `{k}`"))
            .collect();
        if unknown.is_empty() { Ok(()) } else { Err(unknown.join("; ")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gt3_rs_matches_its_published_figures() {
        let spec = CarSpec::porsche_992_gt3_rs();
        assert_eq!(spec.ratios.len(), 7);
        assert_eq!(spec.layout, Layout::RearEngine);
        assert_eq!(spec.drive, Drive::Rwd);
        assert!((spec.torque_at(6300.0) - 465.0).abs() < 0.5, "465 Nm at 6,300 rpm");
        let (kw, rpm) = spec.peak_power();
        assert!((kw - 386.0).abs() < 4.0, "386 kW peak, got {kw}");
        assert!((8000.0..=8800.0).contains(&rpm), "peak power near 8,500 rpm, got {rpm}");
    }

    #[test]
    fn mistakes_are_reported_with_their_line() {
        let broken = PORSCHE_992_GT3_RS.replace("mass_kg = 1450", "mass_kg = heavy");
        assert!(CarSpec::parse(&broken).unwrap_err().contains("car.mass_kg"));
        let unknown = format!("{PORSCHE_992_GT3_RS}\n[car]\nturbo = yes\n");
        assert!(CarSpec::parse(&unknown).unwrap_err().contains("unknown key `car.turbo`"));
        let backwards = PORSCHE_992_GT3_RS.replace("ratios = 3.75 2.381", "ratios = 2.381 3.75");
        assert!(CarSpec::parse(&backwards).unwrap_err().contains("taller"));
    }
}
