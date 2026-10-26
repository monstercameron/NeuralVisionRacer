//! Low-cost car occlusion proxy, fitted from the same reliable Gaussian surfaces used by clearcoat.
//! Seven overlapping ellipsoids follow the scan's longitudinal silhouette instead of its padded draw bounds.
use crate::camera::{self, Mat4};
pub const SEGMENTS: usize = 7;
// to_proxy mat4, sun/strength, receiver-model sun/softness, seven centre/radius pairs.
pub const UNIFORM_FLOATS: usize = 24 + SEGMENTS * 8;
pub const UNIFORM_BYTES: u64 = UNIFORM_FLOATS as u64 * 4;
#[derive(Clone, Debug)]
pub struct Proxy {
    pub bounds: ([f32; 3], [f32; 3]),
    pub ellipsoids: Vec<([f32; 3], [f32; 3])>,
}
fn quantile(v: &mut [f32], fraction: f32) -> f32 {
    v.sort_by(f32::total_cmp);
    v[((v.len() - 1) as f32 * fraction).round() as usize]
}
fn extent(points: &[[f32; 3]], q: f32) -> ([f32; 3], [f32; 3]) {
    let lo = std::array::from_fn(|a| {
        let mut v: Vec<_> = points.iter().map(|p| p[a]).collect();
        quantile(&mut v, q)
    });
    let hi = std::array::from_fn(|a| {
        let mut v: Vec<_> = points.iter().map(|p| p[a]).collect();
        quantile(&mut v, 1.0 - q)
    });
    (lo, hi)
}
impl Proxy {
    pub fn fit(points: &[[f32; 3]], normals: &[u32]) -> Option<Self> {
        let valid: Vec<_> = points
            .iter()
            .enumerate()
            .filter(|(i, p)| {
                p.iter().all(|v| v.is_finite()) && normals.get(*i).is_some_and(|n| n >> 24 >= 64)
            })
            .map(|(_, p)| *p)
            .collect();
        if valid.len() < 32 {
            return None;
        }
        let bounds = extent(&valid, 0.01);
        let span: [f32; 3] = std::array::from_fn(|a| (bounds.1[a] - bounds.0[a]).max(1e-4));
        let axis = (0..3).max_by(|a, b| span[*a].total_cmp(&span[*b])).unwrap();
        let step = span[axis] / SEGMENTS as f32;
        let mut ellipsoids = Vec::new();
        for i in 0..SEGMENTS {
            let along = bounds.0[axis] + (i as f32 + 0.5) * step;
            let slice: Vec<_> = valid
                .iter()
                .filter(|p| (p[axis] - along).abs() < step * 0.80)
                .copied()
                .collect();
            if slice.len() < 8 {
                continue;
            }
            let (lo, hi) = extent(&slice, 0.025);
            let mut center = std::array::from_fn(|a| (lo[a] + hi[a]) * 0.5);
            let mut radius = std::array::from_fn(|a| ((hi[a] - lo[a]) * 0.5).max(span[a] * 0.02));
            center[axis] = along;
            radius[axis] = step * 0.90;
            // Slightly inset the proxy to reduce false self-occlusion on the fitted surface. Cross-sections overlap.
            for a in 0..3 {
                if a != axis {
                    radius[a] *= 0.96;
                }
            }
            ellipsoids.push((center, radius));
        }
        (!ellipsoids.is_empty()).then_some(Self { bounds, ellipsoids })
    }
    pub fn unit_map(&self) -> Mat4 {
        let s = std::array::from_fn::<_, 3, _>(|a| {
            2.0 / (self.bounds.1[a] - self.bounds.0[a]).max(1e-4)
        });
        let c = std::array::from_fn::<_, 3, _>(|a| (self.bounds.0[a] + self.bounds.1[a]) * 0.5);
        [
            s[0],
            0.0,
            0.0,
            0.0,
            0.0,
            s[1],
            0.0,
            0.0,
            0.0,
            0.0,
            s[2],
            0.0,
            -c[0] * s[0],
            -c[1] * s[1],
            -c[2] * s[2],
            1.0,
        ]
    }
    /// Source model to car model, and source-model direction towards the same sun used for the highlight.
    pub fn uniform(
        &self,
        to_car: &Mat4,
        sun_source: [f32; 3],
        strength: f32,
        softness: f32,
        self_only: bool,
    ) -> Vec<u8> {
        let unit = self.unit_map();
        let map = camera::mul(&unit, to_car);
        let sun = direction(&map, sun_source);
        let mut values = vec![0.0; UNIFORM_FLOATS];
        values[..16].copy_from_slice(&map);
        values[16..19].copy_from_slice(&sun);
        values[19] = strength.clamp(0.0, 1.0);
        values[20..23].copy_from_slice(&sun_source);
        values[23] = softness.clamp(0.03, 0.35) * if self_only { -1.0 } else { 1.0 };
        let scales = [unit[0], unit[5], unit[10]];
        for (i, (center, radius)) in self.ellipsoids.iter().enumerate() {
            let offset = 24 + i * 8;
            values[offset..offset + 3].copy_from_slice(&camera::transform_point(&unit, *center));
            values[offset + 3] = 1.0;
            for a in 0..3 {
                values[offset + 4 + a] = radius[a] * scales[a];
            }
        }
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
}
pub fn direction(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|a| m[a] * v[0] + m[a + 4] * v[1] + m[a + 8] * v[2])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reliable_surfaces_fit_the_body_without_floaters() {
        let mut p = Vec::new();
        for z in 0..100 {
            for t in 0..32 {
                let a = t as f32 / 32.0 * std::f32::consts::TAU;
                p.push([a.cos(), a.sin() * 0.5, (z as f32 / 99.0 - 0.5) * 5.0]);
            }
        }
        let mut normals = vec![0xff8080ff; p.len()];
        p.push([100.0; 3]);
        normals.push(0);
        let proxy = Proxy::fit(&p, &normals).unwrap();
        assert_eq!(proxy.ellipsoids.len(), SEGMENTS);
        assert!(proxy.bounds.1.iter().all(|v| *v < 3.0));
        assert!(proxy
            .ellipsoids
            .iter()
            .all(|(_, r)| r.iter().all(|v| v.is_finite() && *v > 0.0)));
        let bytes = proxy.uniform(&camera::IDENTITY, [1.0, 1.0, 0.0], 0.55, 0.1, false);
        assert_eq!(bytes.len() as u64, UNIFORM_BYTES);
    }
    #[test]
    fn insufficient_or_unreliable_surfaces_disable_the_proxy() {
        assert!(Proxy::fit(&[[0.0; 3]; 64], &[0; 64]).is_none());
    }
}
