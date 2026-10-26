//! Authored demo-road collision, independent of the scanned colours and source voxel files.
//! The original fitted geometry remains outside the widened driving corridor.
use crate::svo::WallBox;
#[derive(Clone, Debug)]
pub struct RoadProfile {
    pub knots: Vec<[f32; 3]>,
    pub half_width: f32,
    pub clearance: f32,
    /// Optional [start Z, full Z, half-width] for a gradual terminal shoulder.
    pub end_flare: Option<[f32; 3]>,
}
impl RoadProfile {
    pub fn for_level(path: &str) -> Option<Self> {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../../../specs/road-collision.json"))
                .expect("road collision profiles");
        let p = v["levels"].get(path.trim_end_matches('/').rsplit('/').next()?)?;
        let knots = p["centreline"]
            .as_array()?
            .iter()
            .map(|v| [0, 1, 2].map(|i| v[i].as_f64().expect("road coordinate") as f32))
            .collect::<Vec<_>>();
        assert!(knots.len() >= 2 && knots.windows(2).all(|p| p[1][2] > p[0][2]));
        Some(Self {
            knots,
            half_width: p["half_width_m"].as_f64()? as f32,
            clearance: p["clearance_m"].as_f64()? as f32,
            end_flare: p["end_flare"]
                .as_array()
                .map(|v| [0, 1, 2].map(|i| v[i].as_f64().expect("end flare") as f32)),
        })
    }
    pub fn half_width_at(&self, z: f32) -> f32 {
        let Some([start, end, width]) = self.end_flare else {
            return self.half_width;
        };
        let t = ((z - start) / (end - start).max(0.001)).clamp(0.0, 1.0);
        self.half_width + (width - self.half_width) * t * t * (3.0 - 2.0 * t)
    }
    /// C1 Hermite interpolation preserves the street's gradual grade without five-centimetre voxel steps.
    pub fn centre(&self, z: f32) -> Option<[f32; 3]> {
        if z < self.knots.first()?[2] || z > self.knots.last()?[2] {
            return None;
        }
        let i = self
            .knots
            .partition_point(|p| p[2] < z)
            .saturating_sub(1)
            .min(self.knots.len() - 2);
        let a = self.knots[i];
        let b = self.knots[i + 1];
        let dz = b[2] - a[2];
        let t = (z - a[2]) / dz;
        let slope = |k: usize, axis: usize| {
            let lo = k.saturating_sub(1);
            let hi = (k + 1).min(self.knots.len() - 1);
            (self.knots[hi][axis] - self.knots[lo][axis]) / (self.knots[hi][2] - self.knots[lo][2])
        };
        let eval = |axis| {
            (2.0 * t * t * t - 3.0 * t * t + 1.0) * a[axis]
                + (t * t * t - 2.0 * t * t + t) * dz * slope(i, axis)
                + (-2.0 * t * t * t + 3.0 * t * t) * b[axis]
                + (t * t * t - t * t) * dz * slope(i + 1, axis)
        };
        Some([eval(0), eval(1), z])
    }
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        self.height_with_apron(x, z, 0.0)
    }
    /// Cover a whole boundary mesh cell, so an elevated source corner cannot form a ramp into the lane.
    pub fn height_with_apron(&self, x: f32, z: f32, apron: f32) -> Option<f32> {
        let p = self.centre(z)?;
        ((x - p[0]).abs() <= self.half_width_at(z) + apron.max(0.0)).then_some(p[1])
    }
    /// Fitted boxes have <=20 cm lateral footprints. Reject only road-band intrusions;
    /// the conservative overlap can free at most one such footprint beyond the shoulder.
    pub fn blocks_road(&self, b: &WallBox, origin: [f32; 3]) -> bool {
        let lo = b.min[2] + origin[2];
        let hi = b.max[2] + origin[2];
        let start = self.knots[0][2];
        let end = self.knots.last().unwrap()[2];
        if hi < start || lo > end {
            return false;
        }
        for z in [
            lo.max(start),
            ((lo + hi) * 0.5).clamp(start, end),
            hi.min(end),
        ] {
            if let Some(p) = self.centre(z) {
                let width = self.half_width_at(z);
                if b.max[0] + origin[0] > p[0] - width
                    && b.min[0] + origin[0] < p[0] + width
                    && b.max[1] + origin[1] > p[1] - 0.2
                    && b.min[1] + origin[1] < p[1] + self.clearance
                {
                    return true;
                }
            }
        }
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delft_grade_is_continuous_bounded_and_level_specific() {
        let r = RoadProfile::for_level("levels/trompetstraat-delft").unwrap();
        assert!(RoadProfile::for_level("levels/nmbu-park-03").is_none());
        let mut last = r.centre(-83.0).unwrap();
        for i in 1..=1760 {
            let p = r.centre(-83.0 + i as f32 * 0.1).unwrap();
            assert!(
                (p[1] - last[1]).abs() < 0.006,
                "road grade discontinuity at {p:?}"
            );
            assert!(r
                .height(p[0] + r.half_width_at(p[2]) - 0.01, p[2])
                .is_some());
            assert!(r
                .height(p[0] + r.half_width_at(p[2]) + 0.01, p[2])
                .is_none());
            last = p;
        }
        assert!(r.centre(-83.01).is_none());
        assert!(r.centre(93.01).is_none());
        assert!((r.half_width_at(50.0) - 2.65).abs() < 0.001);
        assert!((r.half_width_at(80.0) - 3.2).abs() < 0.001);
    }
    #[test]
    fn removes_floating_road_clutter_and_preserves_facades_and_high_overhangs() {
        let r = RoadProfile::for_level("trompetstraat-delft").unwrap();
        let p = r.centre(0.0).unwrap();
        let b = WallBox {
            min: [p[0] - 0.1, p[1] + 0.5, -0.1],
            max: [p[0] + 0.1, p[1] + 1.5, 0.1],
        };
        assert!(r.blocks_road(&b, [0.; 3]));
        let wall = WallBox {
            min: [p[0] + 3., b.min[1], -0.1],
            max: [p[0] + 3.2, b.max[1], 0.1],
        };
        assert!(!r.blocks_road(&wall, [0.; 3]));
        assert!(!r.blocks_road(
            &WallBox {
                min: [b.min[0], p[1] + 4., -0.1],
                max: [b.max[0], p[1] + 5., 0.1]
            },
            [0.; 3]
        ));
    }
}
