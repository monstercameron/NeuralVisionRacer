//! Non-blocking foliage contact with bounded, dissipative drag. No rigid impulse/rebound.
use rapier3d::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};
pub const CELL_M: f32 = 0.25;
pub const SENSOR_TAG: u128 = 0x464f4c49414745;
/// Conservative source-colour evidence; brown trunks and neutral masonry do not qualify.
pub fn green([r, g, b]: [f32; 3]) -> bool {
    g > 0.06 && g > r * 1.12 && g > b * 1.10 && g - r > 0.025
}
/// Reject flat green surfaces using volumetric evidence; noisy scans also need reviewed overrides.
#[derive(Default)]
pub struct Evidence {
    total: u32,
    greens: u32,
    sum: [f64; 3],
    outer: [[f64; 3]; 3],
}
impl Evidence {
    pub fn push(&mut self, p: [f32; 3], rgb: [f32; 3]) {
        self.total += 1;
        if !green(rgb) {
            return;
        }
        self.greens += 1;
        for i in 0..3 {
            self.sum[i] += p[i] as f64;
            for j in 0..3 {
                self.outer[i][j] += f64::from(p[i]) * f64::from(p[j]);
            }
        }
    }
    pub fn foliage(&self) -> bool {
        self.greens >= 6 && self.greens * 100 >= self.total * 60 && self.thickness() > 0.014
    }
    /// Smallest covariance eigenvalue converted to metres. Independent of wall orientation.
    pub fn thickness(&self) -> f64 {
        if self.greens < 4 {
            return 0.0;
        }
        let n = f64::from(self.greens);
        let a: [[_; 3]; 3] = std::array::from_fn(|i| {
            std::array::from_fn(|j| self.outer[i][j] / n - self.sum[i] * self.sum[j] / (n * n))
        });
        let q = (a[0][0] + a[1][1] + a[2][2]) / 3.0;
        let p = (((0..3).map(|i| (a[i][i] - q).powi(2)).sum::<f64>()
            + 2.0 * (a[0][1].powi(2) + a[0][2].powi(2) + a[1][2].powi(2)))
            / 6.0)
            .sqrt();
        if p < 1e-12 {
            return q.max(0.0).sqrt();
        }
        let b: [[_; 3]; 3] = std::array::from_fn(|i| {
            std::array::from_fn(|j| (a[i][j] - if i == j { q } else { 0.0 }) / p)
        });
        let det = b[0][0] * (b[1][1] * b[2][2] - b[1][2] * b[2][1])
            - b[0][1] * (b[1][0] * b[2][2] - b[1][2] * b[2][0])
            + b[0][2] * (b[1][0] * b[2][1] - b[1][1] * b[2][0]);
        let phi = ((det / 2.0).clamp(-1.0, 1.0)).acos() / 3.0;
        (q + 2.0 * p * (phi + 2.0 * std::f64::consts::PI / 3.0).cos())
            .max(0.0)
            .sqrt()
    }
}

pub struct MaterialMap {
    cells: BTreeSet<[i32; 3]>,
}
impl MaterialMap {
    pub fn cells(&self) -> impl Iterator<Item = &[i32; 3]> {
        self.cells.iter()
    }
    #[cfg(test)]
    pub(crate) fn from_cells(cells: impl IntoIterator<Item = [i32; 3]>) -> Self {
        Self {
            cells: cells.into_iter().collect(),
        }
    }
    pub fn contains(&self, p: [f32; 3]) -> bool {
        self.cells.contains(&p.map(|v| (v / CELL_M).floor() as i32))
    }
}
/// Split only along material-cell boundaries so a leaf beside masonry cannot soften the whole box.
pub fn split_box(
    b: crate::svo::WallBox,
    origin: [f32; 3],
    map: &MaterialMap,
) -> Vec<(crate::svo::WallBox, bool)> {
    let lo = [0, 1, 2].map(|i| ((b.min[i] + origin[i]) / CELL_M).floor() as i32);
    let hi = [0, 1, 2].map(|i| ((b.max[i] + origin[i] - 1e-5) / CELL_M).floor() as i32);
    let mut any = false;
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                any |= map.cells.contains(&[x, y, z]);
            }
        }
    }
    if !any {
        return vec![(b, false)];
    }
    let mut pieces = Vec::new();
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                let k = [x, y, z];
                let piece = crate::svo::WallBox {
                    min: [0, 1, 2].map(|i| b.min[i].max(k[i] as f32 * CELL_M - origin[i])),
                    max: [0, 1, 2].map(|i| b.max[i].min((k[i] + 1) as f32 * CELL_M - origin[i])),
                };
                if (0..3).all(|i| piece.max[i] - piece.min[i] > 1e-6) {
                    pieces.push((piece, map.cells.contains(&k)));
                }
            }
        }
    }
    pieces
}

pub fn for_level(path: &str) -> Option<&'static MaterialMap> {
    static MAPS: OnceLock<BTreeMap<String, MaterialMap>> = OnceLock::new();
    let maps = MAPS.get_or_init(|| {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../../../specs/foliage-materials.json"))
                .expect("foliage maps");
        assert_eq!(v["cell_m"].as_f64().unwrap() as f32, CELL_M);
        v["levels"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, v)| {
                let cells = v["cells"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| [0, 1, 2].map(|i| p[i].as_i64().unwrap() as i32))
                    .collect();
                (name.clone(), MaterialMap { cells })
            })
            .collect()
    });
    maps.get(path.trim_end_matches('/').rsplit('/').next().unwrap_or(""))
}
/// Rate-limited quadratic drag at the body's centre: horizontal energy only decreases, never reverses.
/// At 20 m/s the default removes 0.8 m/s per second in continuous foliage (roughly 2% speed over 10 m).
pub fn slowed_velocity(v: Vector, dt: f32, drag_per_m: f32, max_decel: f32) -> Vector {
    let speed = v.x.hypot(v.z);
    if speed < 1e-5 || dt <= 0.0 {
        return v;
    }
    let next = (speed / (1.0 + drag_per_m.max(0.0) * speed * dt))
        .max(speed - max_decel.max(0.0) * dt)
        .max(0.0);
    Vector::new(v.x * next / speed, v.y, v.z * next / speed)
}
#[derive(Default)]
pub struct Contact {
    pub active: bool,
    pub sensor_pairs: usize,
    pub lost_energy_j: f64,
    pub last_decel_mps2: f32,
}
impl Contact {
    pub fn step(
        &mut self,
        world: &mut PhysicsWorld,
        body: RigidBodyHandle,
        hull: ColliderHandle,
        dt: f32,
        drag: f32,
        cap: f32,
    ) {
        // Cap at one response regardless of the number of voxels/tiles touched.
        self.sensor_pairs = world
            .intersection_pairs_with(hull)
            .filter(|(_, a, _, b, touch)| {
                *touch && (a.user_data == SENSOR_TAG || b.user_data == SENSOR_TAG)
            })
            .count();
        self.active = self.sensor_pairs > 0;
        self.last_decel_mps2 = 0.0;
        if !self.active {
            return;
        }
        let Some(body) = world.bodies.get_mut(body) else {
            return;
        };
        let before = body.linvel();
        let after = slowed_velocity(before, dt, drag, cap);
        let lost = 0.5
            * body.mass()
            * (before.x * before.x + before.z * before.z - after.x * after.x - after.z * after.z)
                .max(0.0);
        self.lost_energy_j += f64::from(lost);
        self.last_decel_mps2 = (before.x.hypot(before.z) - after.x.hypot(after.z)) / dt.max(1e-6);
        if lost > 0.0 {
            body.apply_impulse((after - before) * body.mass(), true);
        }
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"active":self.active,"sensor_pairs":self.sensor_pairs,"lost_energy_j":self.lost_energy_j,"deceleration_mps2":self.last_decel_mps2,"model":"non-blocking foliage sensor + capped quadratic drag; trunks/unknown material remain rigid"})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn painted_surfaces_at_any_angle_stay_rigid_but_leaf_volume_is_soft() {
        let colour = [0.1, 0.3, 0.12];
        for angle in [0.0_f32, 0.6, 1.2] {
            let mut paint = Evidence::default();
            for x in 0..8 {
                for y in 0..8 {
                    let u = x as f32 * 0.025;
                    let v = y as f32 * 0.025;
                    paint.push(
                        [
                            u * angle.cos() + v * angle.sin(),
                            v,
                            u * angle.sin() - v * angle.cos(),
                        ],
                        colour,
                    );
                }
            }
            assert!(
                !paint.foliage(),
                "green painted plane must remain solid regardless of orientation"
            );
        }
        let mut leaves = Evidence::default();
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    leaves.push([x as f32 * 0.05, y as f32 * 0.05, z as f32 * 0.05], colour);
                }
            }
        }
        assert!(leaves.foliage());
    }
    #[test]
    fn material_classifier_and_boundaries_keep_trunks_and_masonry_solid() {
        assert!(green([0.15, 0.30, 0.09]));
        for rgb in [[0.35, 0.22, 0.1], [0.5, 0.5, 0.5], [0.04, 0.05, 0.03]] {
            assert!(!green(rgb));
        }
        let map = MaterialMap::from_cells([[0, 0, 0]]);
        let b = crate::svo::WallBox {
            min: [0.1, 0.0, 0.0],
            max: [0.4, 0.25, 0.25],
        };
        let pieces = split_box(b, [0.0; 3], &map);
        assert_eq!(pieces.len(), 2);
        assert!(pieces[0].1);
        assert!(!pieces[1].1);
        assert_eq!(pieces[0].0.max[0], pieces[1].0.min[0]);
        let volume: f32 = pieces
            .iter()
            .map(|(b, _)| (0..3).map(|i| b.max[i] - b.min[i]).product::<f32>())
            .sum();
        assert!((volume - 0.3 * 0.25 * 0.25).abs() < 1e-6);
    }
    #[test]
    fn dissipative_drag_is_substep_independent_capped_and_preserves_vertical_motion() {
        let run = |hz: usize| {
            let mut v = Vector::new(-6.0, -3.0, 8.0);
            for _ in 0..hz {
                v = slowed_velocity(v, 1.0 / hz as f32, 0.002, 0.8);
            }
            v
        };
        let reference = run(30);
        for hz in [60, 120, 240] {
            assert!((run(hz) - reference).length() < 0.0001);
        }
        assert_eq!(reference.y, -3.0);
        assert!((reference.x / reference.z + 0.75).abs() < 1e-6);
        for speed in [0.0, 0.01, 10.0, 30.0, 100.0] {
            let next = slowed_velocity(Vector::new(0.0, 4.0, -speed), 1.0 / 120.0, 0.002, 0.8);
            assert!(next.z <= 0.0 && next.z >= -speed);
            assert!(speed - next.z.abs() <= 0.8 / 120.0 + 1e-5);
            assert_eq!(next.y, 4.0);
        }
    }
    fn crossing(sensor: bool, copies: usize) -> (f32, f32, f64, usize) {
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::ZERO;
        world.integration_parameters.dt = 1.0 / 120.0;
        let (body, hull) = world.insert(
            RigidBodyBuilder::dynamic()
                .translation(Vector::new(0.0, 0.0, -5.0))
                .linvel(Vector::new(0.0, 0.0, 20.0))
                .ccd_enabled(true),
            ColliderBuilder::cuboid(0.5, 0.5, 0.5).mass(1500.0),
        );
        for _ in 0..copies {
            world.insert(
                RigidBodyBuilder::fixed(),
                ColliderBuilder::cuboid(2.0, 2.0, 2.0)
                    .sensor(sensor)
                    .user_data(SENSOR_TAG),
            );
        }
        let mut contact = Contact::default();
        let mut hard_hits = 0;
        for _ in 0..60 {
            world.step();
            contact.step(&mut world, body, hull, 1.0 / 120.0, 0.002, 0.8);
            hard_hits += world
                .contact_pairs_with(hull)
                .filter(|p| p.has_any_active_contact())
                .count();
        }
        let b = &world.bodies[body];
        (
            b.translation().z,
            b.linvel().z,
            contact.lost_energy_j,
            hard_hits,
        )
    }
    #[test]
    fn foliage_is_pass_through_with_small_energy_loss_not_a_hard_crash() {
        let free = crossing(true, 0);
        let soft = crossing(true, 1);
        let duplicate = crossing(true, 3);
        let hard = crossing(false, 1);
        eprintln!("foliage crossing (z, speed, lost J, hard contacts): free={free:?}, soft={soft:?}, duplicate={duplicate:?}, hard={hard:?}");
        assert!(soft.0 > 4.5 && soft.1 > 19.5 && soft.1 < free.1);
        assert_eq!(soft.3, 0);
        assert!(soft.2 > 500.0 && soft.2 < 9000.0);
        assert!(
            (soft.1 - duplicate.1).abs() < 1e-5,
            "voxel count must not multiply drag"
        );
        assert!(
            hard.0 < 0.0 && hard.1 < 1.0 && hard.3 > 0,
            "unknown hard geometry must still block"
        );
    }
    #[test]
    fn baked_maps_are_scoped_and_have_supported_cells() {
        assert!(for_level("missing").is_none());
        // Actual noisy painted garage door from the visual review is explicitly protected.
        assert!(!for_level("trompetstraat-delft")
            .unwrap()
            .contains([-2.5, 0.125, 30.5]));
        for name in ["trompetstraat-delft", "nmbu-park-03"] {
            let m = for_level(name).unwrap();
            assert!(m.cells.len() > 10);
            for k in &m.cells {
                assert!(m.contains(k.map(|v| (v as f32 + 0.5) * CELL_M)));
            }
        }
    }
}
