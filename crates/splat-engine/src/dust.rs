//! Ground-material-aware wheel dust. Emit on fixed physics substeps; animate billboards on the GPU.
use crate::tire_marks::TireContact;
use rapier3d::prelude::Vector;
use splat_render::dust::{DustParticle, ParticleKind, CAPACITY};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::OnceLock,
};
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub kind: u8,
    pub dustiness: f32,
    pub colour: [f32; 3],
}
impl Surface {
    pub fn named(kind: u8, sand: bool) -> Self {
        let (dustiness, colour) = match kind {
            b'd' => (
                1.0,
                if sand {
                    [0.80, 0.71, 0.58]
                } else {
                    [0.60, 0.50, 0.37]
                },
            ),
            b'g' => (0.18, [0.60, 0.57, 0.43]),
            b's' => (0.0, [0.77, 0.79, 0.80]),
            b'p' => (0.0, [0.77, 0.79, 0.80]),
            _ => (0.0, [0.77, 0.79, 0.80]),
        };
        Self {
            kind,
            dustiness,
            colour,
        }
    }
    fn name(self) -> &'static str {
        match self.kind {
            b'd' => "dry soil/sand",
            b'g' => "grass",
            b's' => "stone/cobble",
            b'p' => "pavement",
            _ => "unknown",
        }
    }
}
#[derive(Clone, Copy)]
struct Emission {
    surface: u8,
    kind: ParticleKind,
    speed: f32,
    slip: f32,
    lateral: f32,
    heat: f32,
    rate: f32,
}
struct Grid {
    origin: [f32; 2],
    dims: [usize; 2],
    ids: Vec<u8>,
    heights: Vec<i16>,
    fallback: u8,
}
fn maps() -> &'static BTreeMap<String, Grid> {
    static MAPS: OnceLock<BTreeMap<String, Grid>> = OnceLock::new();
    MAPS.get_or_init(|| {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("../../../specs/surface-materials.json"))
                .expect("embedded surface maps");
        value["levels"]
            .as_object()
            .expect("levels")
            .iter()
            .map(|(name, v)| {
                let ids = v["ids"].as_str().unwrap().as_bytes().to_vec();
                let dims = [0, 1].map(|i| v["dims"][i].as_u64().unwrap() as usize);
                let heights = v["ground_y_dm"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|y| y.as_i64().unwrap() as i16)
                    .collect::<Vec<_>>();
                assert_eq!(ids.len(), dims[0] * dims[1]);
                assert_eq!(heights.len(), ids.len());
                (
                    name.clone(),
                    Grid {
                        origin: [0, 1].map(|i| v["origin"][i].as_f64().unwrap() as f32),
                        dims,
                        ids,
                        heights,
                        fallback: v["fallback"].as_str().unwrap().as_bytes()[0],
                    },
                )
            })
            .collect()
    })
}
#[derive(Default)]
pub struct Dust {
    pub particles: VecDeque<DustParticle>,
    pub time: f64,
    pub revision: u64,
    pub strength: f32,
    level: String,
    road: Option<crate::road::RoadProfile>,
    heat: [f32; 4],
    credit: [f32; 4],
    previous: [Option<Vector>; 4],
    seed: u32,
    clock: Option<fn() -> f64>,
    metrics: splat_render::metrics::Metrics,
    emitted: u64,
    evicted: u64,
    latest: [Option<Emission>; 4],
}
impl Dust {
    pub fn set_level(&mut self, path: &str) {
        let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        if self.level != name {
            self.level = name.to_owned();
            self.road = crate::road::RoadProfile::for_level(name);
            self.clear();
        }
    }
    pub fn set_clock(&mut self, clock: fn() -> f64) {
        self.clock = Some(clock);
    }
    pub fn clear(&mut self) {
        self.particles.clear();
        self.credit = [0.0; 4];
        self.heat = [0.0; 4];
        self.previous = [None; 4];
        self.latest = [None; 4];
        self.revision += 1;
    }
    pub fn surface_at(&self, p: Vector) -> Surface {
        // The authored Delft corridor is a reviewed cobbled street, even where the source floor had gaps.
        if self
            .road
            .as_ref()
            .and_then(|r| r.height(p.x, p.z))
            .is_some_and(|y| (p.y - y).abs() < 0.35)
        {
            return Surface::named(b's', false);
        }
        let sand = self.level == "saburi-alley-yazd";
        let Some(grid) = maps().get(&self.level) else {
            return Surface::named(b'?', false);
        };
        let (x, z) = (
            (p.x - grid.origin[0]).floor() as isize,
            (p.z - grid.origin[1]).floor() as isize,
        );
        if x < 0 || z < 0 || x >= grid.dims[0] as isize || z >= grid.dims[1] as isize {
            return Surface::named(b'?', false);
        }
        let i = x as usize * grid.dims[1] + z as usize;
        let id = if grid.ids[i] == b'.' {
            grid.fallback
        } else if (p.y - grid.heights[i] as f32 * 0.1).abs() > 0.65 {
            b'?'
        } else {
            grid.ids[i]
        };
        Surface::named(id, sand)
    }
    fn random(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.seed >> 8) as f32 / 16777216.0
    }
    pub fn step(&mut self, contacts: [Option<TireContact>; 4], dt: f32) {
        let started = self.clock.map(|c| c());
        let surfaces = contacts.map(|c| c.map(|c| self.surface_at(c.point)));
        self.step_surfaces(contacts, surfaces, dt);
        if let (Some(clock), Some(start)) = (self.clock, started) {
            self.metrics.record("generate", clock() - start);
        }
    }
    fn step_surfaces(
        &mut self,
        contacts: [Option<TireContact>; 4],
        surfaces: [Option<Surface>; 4],
        dt: f32,
    ) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        self.time += f64::from(dt);
        // Lifetimes vary: expiry is not ordered by birth time.
        let old = self.particles.len();
        self.particles
            .retain(|p| self.time - f64::from(p.born) < f64::from(p.life));
        if old != self.particles.len() {
            self.revision += 1;
        }
        self.latest = [None; 4];
        for i in 0..4 {
            let Some(c) = contacts[i].filter(|c| {
                c.load > 0.025
                    && c.normal.is_finite()
                    && c.normal.y > 0.35
                    && c.point.is_finite()
                    && c.angular_speed.is_finite()
                    && c.longitudinal_speed.is_finite()
                    && c.lateral_speed.is_finite()
            }) else {
                self.credit[i] = 0.0;
                self.heat[i] = 0.0;
                self.previous[i] = None;
                continue;
            };
            let surface = surfaces[i].unwrap();
            let speed = c.longitudinal_speed.hypot(c.lateral_speed);
            let slip = (c.angular_speed * c.radius - c.longitudinal_speed).abs();
            let lateral = c.lateral_speed.abs();
            let demand =
                ((slip - 0.9).max(0.0) * 0.10 + (lateral - 0.7).max(0.0) * 0.18).clamp(0.0, 1.0);
            // A short thermal envelope rejects tiny grip corrections and keeps bursts soft.
            let tau = if demand > self.heat[i] { 0.25 } else { 0.45 };
            self.heat[i] += (demand - self.heat[i]) * (1.0 - (-dt / tau).exp());
            let kind = if surface.dustiness > 0.0 {
                ParticleKind::Dust
            } else {
                ParticleKind::TireSmoke
            };
            let load = c.load.clamp(0.0, 1.5).sqrt().min(1.1);
            let rate = if surface.kind == b'?' {
                0.0
            } else if kind == ParticleKind::TireSmoke {
                // No speed-only smoke on tarmac/cobbles. Loaded slip/drift/locked braking warms rubber.
                (24.0 * self.heat[i].powf(1.3) * load).min(24.0)
            } else {
                (((speed - 0.8).max(0.0) * 1.6 + (slip - 0.75).max(0.0) * 2.5 + lateral * 1.5)
                    * surface.dustiness
                    * load)
                    .min(60.0)
            };
            self.latest[i] = Some(Emission {
                surface: surface.kind,
                kind,
                speed,
                slip,
                lateral,
                heat: self.heat[i],
                rate,
            });
            let jumped = self.previous[i].is_some_and(|p| (p - c.point).length() > 3.0);
            if jumped {
                self.credit[i] = 0.0;
                self.heat[i] = 0.0;
                self.previous[i] = Some(c.point);
                continue;
            }
            let start = self.previous[i].unwrap_or(c.point);
            self.previous[i] = Some(c.point);
            if self.strength <= 0.0 || rate <= 0.0 {
                self.credit[i] = 0.0;
                continue;
            }
            self.credit[i] += rate * dt;
            let count = (self.credit[i].floor() as usize).min(32);
            self.credit[i] -= count as f32;
            for n in 0..count {
                let seed = self.random();
                let a = self.random() - 0.5;
                let b = self.random() - 0.5;
                let normal = c.normal.normalize_or_zero();
                let side = c.forward.cross(normal).normalize_or_zero();
                let origin = start.lerp(c.point, (n as f32 + 0.5) / count as f32)
                    + normal * 0.07
                    + side * (a * c.width);
                let smoke = kind == ParticleKind::TireSmoke;
                let rise = if smoke {
                    0.35 + seed * 0.25
                } else {
                    0.25 + seed * 0.25
                };
                // A trace of vehicle entrainment and a small fixed breeze; world-space particles stay behind.
                let velocity = normal * rise
                    - c.forward
                        * (c.longitudinal_speed.signum()
                            * (speed * 0.035 + slip.min(25.0) * 0.025))
                    + side * (b * (0.30 + speed * 0.012))
                    + Vector::new(0.12, 0.0, 0.05);
                let particle = DustParticle {
                    kind,
                    origin: origin.to_array(),
                    velocity: velocity.to_array(),
                    normal: normal.to_array(),
                    born: self.time as f32,
                    life: if smoke {
                        1.45 + seed * 0.45
                    } else {
                        1.65 + seed * 0.45
                    },
                    size: if smoke {
                        0.14 + seed * 0.04
                    } else {
                        0.14 + (speed.min(30.0) / 30.0) * 0.11 + seed * 0.035
                    },
                    opacity: if smoke {
                        0.085 + seed * 0.025
                    } else {
                        0.16 + seed * 0.045
                    },
                    colour: surface.colour,
                    seed,
                };
                if self.particles.len() == CAPACITY {
                    self.particles.pop_front();
                    self.evicted += 1;
                }
                self.particles.push_back(particle);
                self.emitted += 1;
                self.revision += 1;
            }
        }
    }
    pub fn profile_json(&self) -> serde_json::Value {
        serde_json::json!({"active":self.particles.len(),"ground_dust":self.particles.iter().filter(|p|p.kind==ParticleKind::Dust).count(),"tire_smoke":self.particles.iter().filter(|p|p.kind==ParticleKind::TireSmoke).count(),"capacity":CAPACITY,"emitted":self.emitted,"evicted":self.evicted,
            "simulation_seconds":self.time,"cpu":self.metrics.json(),"material_source":"height-filtered source colour inference and authored level fallback; 1 m grid with reviewed Delft cobble corridor",
            "level":self.level,"wheels":self.latest.map(|m|m.map(|e|serde_json::json!({"material":Surface::named(e.surface,false).name(),"effect":e.kind.name(),"road_mps":e.speed,"slip_mps":e.slip,"lateral_mps":e.lateral,"rubber_heat":e.heat,"particles_per_s":e.rate})))})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn contact(speed: f32, spin: f32) -> TireContact {
        TireContact {
            point: Vector::ZERO,
            normal: Vector::Y,
            forward: Vector::Z,
            radius: 0.35,
            width: 0.3,
            load: 1.0,
            longitudinal_speed: speed,
            angular_speed: spin / 0.35,
            ..Default::default()
        }
    }
    fn run(hz: usize, speed: f32, spin: f32, kind: u8) -> Dust {
        let mut d = Dust {
            strength: 1.0,
            ..Default::default()
        };
        for _ in 0..hz {
            d.step_surfaces(
                [Some(contact(speed, spin)), None, None, None],
                [Some(Surface::named(kind, true)), None, None, None],
                1.0 / hz as f32,
            );
        }
        d
    }
    #[test]
    fn emission_tracks_speed_slip_material_and_substeps() {
        assert!(run(180, 0.0, 0.0, b'd').particles.is_empty());
        let soil = run(180, 20.0, 20.0, b'd');
        let paved = run(180, 20.0, 20.0, b'p');
        assert!(soil.emitted > paved.emitted * 15);
        assert!(soil.emitted > run(180, 5.0, 5.0, b'd').emitted * 3);
        assert!(
            run(180, 0.0, 20.0, b'd').emitted > 30,
            "blocked-wheel burnout on dirt"
        );
        assert!(soil.emitted.abs_diff(run(60, 20.0, 20.0, b'd').emitted) <= 1);
    }
    #[test]
    fn streets_smoke_only_with_slip_and_loose_floors_raise_coloured_dust() {
        for kind in [b'p', b's'] {
            assert!(
                run(180, 20.0, 20.0, kind).particles.is_empty(),
                "ordinary street rolling stays clean"
            );
            let smoke = run(180, 0.0, 20.0, kind);
            assert!(
                smoke.emitted >= 15 && smoke.emitted <= 24,
                "a restrained slip-driven smoke rate"
            );
            assert!(smoke
                .particles
                .iter()
                .all(|p| p.kind == ParticleKind::TireSmoke
                    && p.opacity <= 0.111
                    && p.colour[2] >= p.colour[0]));
            assert!(smoke.emitted.abs_diff(run(60, 0.0, 20.0, kind).emitted) <= 1);
        }
        let soil = run(180, 20.0, 20.0, b'd');
        assert!(
            soil.emitted > 20
                && soil
                    .particles
                    .iter()
                    .all(|p| p.kind == ParticleKind::Dust && p.colour[0] > p.colour[2])
        );
        assert!(run(180, 20.0, 20.0, b'g').emitted < soil.emitted / 3);
        assert!(
            run(180, 0.0, 20.0, b'?').particles.is_empty(),
            "unclassified geometry must not generate arbitrary clouds"
        );
    }
    #[test]
    fn braking_and_drifting_smoke_while_tiny_grip_corrections_do_not() {
        let simulate = |c: TireContact| {
            let mut d = Dust {
                strength: 1.0,
                ..Default::default()
            };
            for _ in 0..180 {
                d.step_surfaces(
                    [Some(c), None, None, None],
                    [Some(Surface::named(b's', false)), None, None, None],
                    1.0 / 180.0,
                );
            }
            d
        };
        let small = contact(20.0, 20.4);
        assert!(simulate(small).particles.is_empty());
        let locked = TireContact {
            locked: true,
            ..contact(15.0, 0.0)
        };
        assert!(simulate(locked).emitted > 10);
        let drift = TireContact {
            lateral_speed: 4.0,
            ..contact(15.0, 15.0)
        };
        let mut d = simulate(drift);
        assert!(d.emitted > 5);
        let before = d.emitted;
        for _ in 0..180 {
            d.step_surfaces(
                [Some(contact(15.0, 15.0)), None, None, None],
                [Some(Surface::named(b's', false)), None, None, None],
                1.0 / 180.0,
            );
        }
        assert!(
            d.emitted - before < 10,
            "heat fades instead of making a lingering street fog"
        );
        let before = d.emitted;
        d.step_surfaces([None; 4], [None; 4], 0.1);
        assert_eq!(before, d.emitted);
    }
    #[test]
    fn air_pause_expiry_reset_and_capacity_are_bounded() {
        let mut d = run(180, 20.0, 20.0, b'd');
        let before = d.time;
        d.step([None; 4], 0.0);
        assert_eq!(d.time, before);
        let emitted = d.emitted;
        for _ in 0..600 {
            d.step([None; 4], 1.0 / 180.0);
        }
        assert_eq!(emitted, d.emitted);
        assert!(d.particles.is_empty());
        for _ in 0..1000 {
            d.step_surfaces(
                [Some(contact(100.0, 150.0)); 4],
                [Some(Surface::named(b'd', true)); 4],
                0.1,
            );
        }
        assert!(d.particles.len() <= CAPACITY);
        let p = *d.particles.back().unwrap();
        d.particles = std::iter::repeat_n(p, CAPACITY).collect();
        let evicted = d.evicted;
        d.step_surfaces(
            [Some(contact(100.0, 150.0)); 4],
            [Some(Surface::named(b'd', true)); 4],
            0.1,
        );
        assert_eq!(d.particles.len(), CAPACITY);
        assert!(d.evicted > evicted);
        d.clear();
        assert!(d.particles.is_empty());
    }
    #[test]
    fn baked_material_maps_have_consistent_dimensions_and_separate_grass() {
        assert_eq!(maps().len(), 4);
        let park = &maps()["nmbu-park-03"];
        assert!(park.ids.contains(&b'g') && park.ids.contains(&b'p'));
        let mut d = Dust::default();
        d.set_level("levels/saburi-alley-yazd");
        assert_eq!(d.surface_at(Vector::new(-0.725, 0.05, -5.775)).kind, b'd');
        assert_eq!(
            d.surface_at(Vector::new(-0.725, 5.05, -5.775)).kind,
            b'?',
            "a ground map must not label a roof as soil"
        );
    }
}
