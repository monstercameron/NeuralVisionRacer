//! Fits four rigid splat selections to the configured car, without changing/recompressing its SOG textures.
//! The scan's alignment/scale and body centroid are explicitly inverted for the shader's SOG-space poses.
use crate::{drive::WheelVisualState, Engine};
use splat_render::{
    camera::{self, Mat4},
    sog,
    wheel::{self, PartPose, WheelAnimation},
};

#[derive(Debug)]
pub struct WheelRig {
    pub parts: Vec<u32>,
    pub hubs: [[f32; 3]; 4],
    pub body_hubs: [[f32; 3]; 4],
    axle: [f32; 3],
    up: [f32; 3],
    inverse_scale: f32,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] - b[i])
}
fn direction(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| m[i] * v[0] + m[4 + i] * v[1] + m[8 + i] * v[2])
}
fn normalized(v: [f32; 3]) -> [f32; 3] {
    let n = dot(v, v).sqrt().max(1e-6);
    v.map(|a| a / n)
}

/// Refine an approximate hub against the metallic rim ring, balancing uneven scan density by angle.
fn rim_center(points: &[[f32; 2]], tyre_radius: f32) -> Option<[f32; 2]> {
    let mut center = [0.0f32; 2];
    for _ in 0..6 {
        let ring: Vec<_> = points
            .iter()
            .filter(|p| {
                let radius = (p[0] - center[0]).hypot(p[1] - center[1]);
                radius > tyre_radius * 0.63 && radius < tyre_radius * 0.845
            })
            .collect();
        if ring.len() < 100 {
            return None;
        }
        let bin = |p: &&[f32; 2]| {
            (((p[1] - center[1]).atan2(p[0] - center[0]) + std::f32::consts::PI) * 32.0
                / std::f32::consts::TAU) as usize
                % 32
        };
        let mut counts = [0usize; 32];
        for p in &ring {
            counts[bin(p)] += 1;
        }
        if counts.iter().filter(|n| **n > 0).count() < 24 {
            return None;
        }
        let mut m = [[0.0f64; 4]; 3];
        for p in ring {
            let weight = 1.0 / counts[bin(&p)] as f64;
            let v = [f64::from(p[0]), f64::from(p[1]), 1.0];
            let squared = v[0] * v[0] + v[1] * v[1];
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += weight * v[i] * v[j];
                }
                m[i][3] += weight * v[i] * squared;
            }
        }
        for i in 0..3 {
            let pivot = (i..3)
                .max_by(|a, b| m[*a][i].abs().total_cmp(&m[*b][i].abs()))
                .unwrap();
            m.swap(i, pivot);
            let divisor = m[i][i];
            if divisor.abs() < 1e-10 {
                return None;
            }
            for j in i..4 {
                m[i][j] /= divisor;
            }
            for k in 0..3 {
                if k == i {
                    continue;
                }
                let factor = m[k][i];
                for j in i..4 {
                    m[k][j] -= factor * m[i][j];
                }
            }
        }
        center = [(m[0][3] * 0.5) as f32, (m[1][3] * 0.5) as f32];
        let radius = (m[2][3] + f64::from(center[0]).powi(2) + f64::from(center[1]).powi(2)).sqrt();
        if !radius.is_finite()
            || radius < f64::from(tyre_radius) * 0.55
            || radius > f64::from(tyre_radius) * 0.95
            || center[0].hypot(center[1]) > 0.08
        {
            return None;
        }
    }
    Some(center)
}

impl WheelRig {
    pub fn fit(engine: &Engine, bundle: &sog::Bundle) -> Option<Self> {
        let geometry = engine.physics.wheel_geometry()?;
        let vehicle = engine.physics.vehicle()?;
        let spec = &engine.physics.car()?.spec;
        let inverse = camera::inverse(&engine.model)?;
        let centers = sog::centres(bundle);
        if centers.len() < 100 {
            return None;
        }
        let points: Vec<_> = centers
            .iter()
            .map(|p| sub(camera::transform_point(&engine.model, *p), vehicle.centroid))
            .collect();
        let right = geometry.right.to_array();
        let forward = geometry.forward.to_array();
        let mut lateral: Vec<_> = points.iter().map(|p| dot(*p, right)).collect();
        lateral.sort_by(f32::total_cmp);
        // A point-count centroid is biased towards the scanned side. Wheel track is about the visual body's
        // centreline (robust width midpoint), not necessarily about that centroid.
        let at = |q: f32| lateral[((lateral.len() - 1) as f32 * q) as usize];
        let middle = (at(0.01) + at(0.99)) * 0.5;
        let radii = [
            spec.front_radius_m,
            spec.front_radius_m,
            spec.rear_radius_m,
            spec.rear_radius_m,
        ];
        let width = |s: &str, fallback: f32| {
            s.split('/')
                .next()
                .and_then(|s| s.parse::<f32>().ok())
                .map_or(fallback, |w| w * 0.001)
        };
        let widths = [
            width(&spec.tyre_front, 0.275),
            width(&spec.tyre_front, 0.275),
            width(&spec.tyre_rear, 0.335),
            width(&spec.tyre_rear, 0.335),
        ];
        let mut body_hubs = std::array::from_fn(|i| {
            let front = i < 2;
            let along = if front {
                geometry.front_axle
            } else {
                geometry.rear_axle
            };
            let track = if front {
                spec.track_front_m
            } else {
                spec.track_rear_m
            };
            let across = middle
                + if i % 2 == 0 {
                    -track * 0.5
                } else {
                    track * 0.5
                };
            [0, 1, 2].map(|a| {
                forward[a] * along
                    + right[a] * across
                    + if a == 1 {
                        geometry.bottom + radii[i]
                    } else {
                        0.0
                    }
            })
        });
        let sh0 = &bundle.textures["sh0"].rgba;
        let scales = &bundle.textures["scales"].rgba;
        let model_scale = dot(
            direction(&engine.model, [1.0, 0.0, 0.0]),
            direction(&engine.model, [1.0, 0.0, 0.0]),
        )
        .sqrt();
        let rgb = |i: usize| {
            [0, 1, 2].map(|c| 0.5 + 0.2820948 * bundle.meta.sh0_codebook[sh0[i * 4 + c] as usize])
        };
        for i in 0..4 {
            let side = if i % 2 == 0 { -1.0 } else { 1.0 };
            let ring: Vec<_> = points
                .iter()
                .enumerate()
                .filter_map(|(index, p)| {
                    let d = sub(*p, body_hubs[i]);
                    let outward = dot(d, right) * side;
                    let along = dot(d, forward);
                    if outward < widths[i] * 0.4
                        || outward > widths[i] * 0.5 + 0.10
                        || along.hypot(d[1]) > radii[i] * 1.1
                    {
                        return None;
                    }
                    let color = rgb(index);
                    let low = color.into_iter().fold(f32::INFINITY, f32::min);
                    let high = color.into_iter().fold(f32::NEG_INFINITY, f32::max);
                    if low < 0.2 || high - low > 0.18 {
                        return None;
                    }
                    let extent = (0..3)
                        .map(|c| {
                            bundle.meta.scales_codebook[scales[index * 4 + c] as usize].exp()
                                * model_scale
                        })
                        .fold(0.0f32, f32::max);
                    (extent < 0.03).then_some([along, d[1]])
                })
                .collect();
            if let Some(offset) = rim_center(&ring, radii[i]) {
                for a in 0..3 {
                    body_hubs[i][a] +=
                        forward[a] * offset[0] + if a == 1 { offset[1] } else { 0.0 };
                }
            }
        }
        let hubs = body_hubs.map(|p| {
            camera::transform_point(&inverse, [0, 1, 2].map(|i| p[i] + vehicle.centroid[i]))
        });
        let parts = points
            .iter()
            .enumerate()
            .map(|(index, p)| {
                // Large reconstruction blobs overlap the fender/body even when their centres lie inside a wheel.
                // Preserve them with the body instead of swinging those reconstruction artefacts around an axle.
                let extent = (0..3)
                    .map(|c| {
                        bundle.meta.scales_codebook[scales[index * 4 + c] as usize].exp()
                            * model_scale
                    })
                    .fold(0.0f32, f32::max);
                for i in 0..4 {
                    let d = sub(*p, body_hubs[i]);
                    let across = dot(d, right);
                    let along = dot(d, forward);
                    let radial = (along * along + d[1] * d[1]).sqrt();
                    // The scanned rim lip protrudes past the nominal tyre half-width. A little
                    // extra room inside the rim includes the outer spokes without catching the fender.
                    let axial_margin = if radial < radii[i] * 0.86 {
                        0.07
                    } else {
                        0.025
                    };
                    if across.abs() <= widths[i] * 0.5 + axial_margin && radial <= radii[i] * 1.01 {
                        // Thin, long rim spokes exceed the ordinary blob cutoff. Include them inside
                        // the rim; keep the stricter limit near the tyre/fender boundary.
                        if extent > 0.06 && (radial > radii[i] * 0.86 || extent > 0.16) {
                            continue;
                        }
                        let [r, g, b] = rgb(index);
                        // The Porsche's yellow brake calipers do not spin. Keep the small central wheel lock with
                        // the rim, and restrict this colour cue to the disc annulus inside the cylinder.
                        let caliper = radial > 0.09
                            && radial < radii[i] * 0.83
                            && r > b * 1.5
                            && g > b * 1.3
                            && r > 0.12
                            && g > 0.10;
                        return i as u32 + if caliper { 5 } else { 1 };
                    }
                }
                0
            })
            .collect();
        Some(Self {
            parts,
            hubs,
            body_hubs,
            axle: normalized(direction(&inverse, right)),
            up: normalized(direction(&inverse, [0.0, 1.0, 0.0])),
            inverse_scale: dot(
                direction(&inverse, [1.0, 0.0, 0.0]),
                direction(&inverse, [1.0, 0.0, 0.0]),
            )
            .sqrt(),
        })
    }

    pub fn animation(&self, states: [WheelVisualState; 4], debug: bool) -> WheelAnimation {
        let mut frame = WheelAnimation {
            enabled: true,
            debug,
            ..Default::default()
        };
        for (i, state) in states.iter().enumerate() {
            let steer = wheel::axis_angle(self.up, -state.steering);
            // Follow the full integrated rolling phase. Wrap only for the GPU quaternion;
            // a sine/bounded displacement reverses direction and makes the wheel rock.
            // Centres and in-plane Gaussian orientation co-rotate without changing axle thickness.
            let angle = (-state.spin).rem_euclid(std::f64::consts::TAU) as f32;
            let spin = wheel::axis_angle(self.axle, angle);
            let travel = self.up.map(|a| a * state.travel * self.inverse_scale);
            frame.parts[i + 1] =
                PartPose::around(self.hubs[i], wheel::multiply(steer, spin), travel);
            frame.parts[i + 5] = PartPose::around(self.hubs[i], steer, travel);
        }
        frame
    }

    pub fn counts(&self) -> [usize; 9] {
        let mut counts = [0; 9];
        for part in &self.parts {
            counts[*part as usize] += 1;
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scanned_rim_refines_an_offset_pivot_without_scan_density_bias() {
        let expected = [-0.032f32, -0.006];
        let mut points = Vec::new();
        for i in 0..640 {
            let angle = i as f32 * std::f32::consts::TAU / 640.0;
            let radius = 0.260 + 0.002 * (angle * 7.0).sin();
            let point = [
                expected[0] + radius * angle.cos(),
                expected[1] + radius * angle.sin(),
            ];
            points.push(point);
            if i < 160 {
                points.extend([point; 5]);
            }
        }
        let center = rim_center(&points, 0.350).unwrap();
        assert!((center[0] - expected[0]).hypot(center[1] - expected[1]) < 0.001);
        assert!(rim_center(&points[..40], 0.350).is_none());
    }

    #[test]
    fn calipers_follow_steering_and_suspension_but_never_axle_spin() {
        let rig = WheelRig {
            parts: vec![0, 1, 5],
            hubs: [[0.0; 3]; 4],
            body_hubs: [[0.0; 3]; 4],
            axle: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            inverse_scale: 1.0,
        };
        let states = [WheelVisualState {
            spin: std::f64::consts::FRAC_PI_2,
            angular_speed: 100.0,
            travel: 0.04,
            ..Default::default()
        }; 4];
        let frame = rig.animation(states, false);
        let caliper = frame.parts[5].point([0.0, 1.0, 0.0]);
        assert!((caliper[1] - 1.04).abs() < 1e-6);
        let rim = frame.parts[1].point([0.0, 1.0, 0.0]);
        assert!((rim[2] + 1.0).abs() < 1e-5);
        assert_eq!(frame.parts[0], PartPose::default());
        // The hub and axial coordinate stay fixed throughout continuous rotation.
        assert_eq!(frame.parts[1].point([0.0; 3]), [0.0, 0.04, 0.0]);
        assert!((frame.parts[1].point([0.12, 0.2, -0.1])[0] - 0.12).abs() < 1e-6);
        assert_eq!(frame.parts[5].rotation, PartPose::default().rotation);
        let stopped = states.map(|s| WheelVisualState {
            angular_speed: 0.0,
            ..s
        });
        assert_eq!(
            rig.animation(stopped, false).parts[1],
            frame.parts[1],
            "stopping retains the wheel angle"
        );
        let mut steered = states;
        steered[0].steering = 0.3;
        assert_ne!(
            rig.animation(steered, false).parts[5].rotation,
            frame.parts[5].rotation
        );
        assert_eq!(
            rig.animation(steered, false).parts[7].rotation,
            frame.parts[7].rotation
        );
        // An asymmetric rim point must complete entire revolutions, always in the same direction.
        let mut previous = [0.0, 1.0, 0.0];
        for step in 1..=128 {
            let phase = step as f64 * std::f64::consts::PI / 8.0;
            let state = [WheelVisualState {
                spin: phase,
                angular_speed: 10.0,
                ..Default::default()
            }; 4];
            let p = rig.animation(state, false).parts[1].point([0.12, 1.0, 0.0]);
            assert!((p[0] - 0.12).abs() < 1e-6);
            assert!((p[1] - phase.cos() as f32).abs() < 1e-5);
            assert!((p[2] + phase.sin() as f32).abs() < 1e-5);
            assert!(
                previous[1] * p[2] - previous[2] * p[1] < 0.0,
                "rotation reversed at phase {phase}"
            );
            // Co-rotating an anisotropic Gaussian does not expand its variance along the axle.
            let source = wheel::axis_angle(normalized([1.0, 2.0, 3.0]), 0.7);
            let combined = wheel::multiply(rig.animation(state, false).parts[1].rotation, source);
            let axial_variance = |q| {
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                    .into_iter()
                    .zip([0.003f32, 0.02, 0.1])
                    .map(|(basis, scale)| (wheel::rotate(q, basis)[0] * scale).powi(2))
                    .sum::<f32>()
            };
            assert!((axial_variance(source) - axial_variance(combined)).abs() < 1e-8);
            previous = p;
        }
    }
}
