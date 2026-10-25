//! Squares a scanned car up with the axes. A scan rarely sits exactly along an axis: a few degrees of yaw make the
//! car look wider than it is, put the wheels off the body's line and widen its collider into the walls.
//!
//! 1. **Yaw**: the principal axis of the splats seen from above (the car's length) is turned onto z.
//! 2. **Level**: the lowest splats in each quarter (the tyre contact patches) are brought to one height, removing
//!    pitch and roll.
//!
//! Stray splats around the car are trimmed first (the farthest 2% from the middle for the yaw; the outer 2% per axis,
//! once squared up, for the level), so they do not steer the fit.

use splat_render::camera::{self, Mat4};

/// The rotation that squares `points` (already upright) up with the axes: length along z, tyres level.
pub fn align(points: &[[f32; 3]]) -> Mat4 {
    if core(points, 0.02).len() < 100 {
        return camera::IDENTITY;
    }
    // Yaw from the xz covariance of the splats nearest the middle. The trim is by distance, which does not depend
    // on the car's turn; trimming to an axis-aligned box would bias a slightly turned car towards that box's axes.
    let yaw = principal_yaw(&nearest(points, 0.98));
    let turn = rotation_y(yaw);
    let turned: Vec<[f32; 3]> = core(&points.iter().map(|p| camera::transform_point(&turn, *p)).collect::<Vec<_>>(), 0.02);
    let n = turned.len() as f32;

    // Level: the lowest 1% of splats in each quarter (front/rear × left/right) are where the tyres touch. Their mean
    // positions give the slope to remove, along z (pitch) and across x (roll).
    let (cx, cz) = turned.iter().fold((0.0, 0.0), |(x, z), p| (x + p[0] / n, z + p[2] / n));
    let low = |front: bool, left: bool| -> [f32; 3] {
        let mut quarter: Vec<[f32; 3]> = turned.iter().filter(|p| (p[2] < cz) == front && (p[0] < cx) == left).copied().collect();
        quarter.sort_by(|a, b| a[1].total_cmp(&b[1]));
        quarter.truncate((quarter.len() / 100).max(1));
        let k = quarter.len() as f32;
        quarter.iter().fold([0.0; 3], |m, p| [m[0] + p[0] / k, m[1] + p[1] / k, m[2] + p[2] / k])
    };
    let mid = |a: [f32; 3], b: [f32; 3]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0];
    let (fl, fr, rl, rr) = (low(true, true), low(true, false), low(false, true), low(false, false));
    let (front, rear, left, right) = (mid(fl, fr), mid(rl, rr), mid(fl, rl), mid(fr, rr));
    // Small-angle pitch (about x) and roll (about z) that bring the rear and right patches to the front and left ones.
    let pitch = ((rear[1] - front[1]) / (rear[2] - front[2])).atan();
    let roll = ((right[1] - left[1]) / (right[0] - left[0])).atan();
    let level = camera::mul(&camera::rotation_x(pitch), &rotation_z(-roll));
    camera::mul(&level, &turn)
}

/// The turn about +y that puts the major axis of `points` (seen from above) on z, folded into ±90°.
fn principal_yaw(points: &[[f32; 3]]) -> f32 {
    let n = points.len().max(1) as f32;
    let (mx, mz) = points.iter().fold((0.0, 0.0), |(x, z), p| (x + p[0] / n, z + p[2] / n));
    let (mut sxx, mut szz, mut sxz) = (0.0f32, 0.0f32, 0.0f32);
    for p in points {
        let (dx, dz) = (p[0] - mx, p[2] - mz);
        sxx += dx * dx;
        szz += dz * dz;
        sxz += dx * dz;
    }
    // Angle `a` of the major axis from +x. `rotation_y(t)` turns the direction (cos a, sin a) in (x, z) to angle
    // a − t, so t = a − 90° lands it on +z.
    let mut yaw = 0.5 * (2.0 * sxz).atan2(sxx - szz) - std::f32::consts::FRAC_PI_2;
    while yaw > std::f32::consts::FRAC_PI_2 {
        yaw -= std::f32::consts::PI;
    }
    while yaw <= -std::f32::consts::FRAC_PI_2 {
        yaw += std::f32::consts::PI;
    }
    yaw
}

/// The `keep` share of `points` nearest their median centre, seen from above.
fn nearest(points: &[[f32; 3]], keep: f32) -> Vec<[f32; 3]> {
    let median = |axis: usize| {
        let mut v: Vec<f32> = points.iter().map(|p| p[axis]).collect();
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let (mx, mz) = (median(0), median(2));
    let mut by_distance: Vec<(f32, [f32; 3])> = points.iter().map(|p| ((p[0] - mx).powi(2) + (p[2] - mz).powi(2), *p)).collect();
    by_distance.sort_by(|a, b| a.0.total_cmp(&b.0));
    by_distance.truncate((points.len() as f32 * keep) as usize);
    by_distance.into_iter().map(|(_, p)| p).collect()
}

/// Points inside the per-axis `q`..`1 - q` percentile box.
fn core(points: &[[f32; 3]], q: f32) -> Vec<[f32; 3]> {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for axis in 0..3 {
        let mut v: Vec<f32> = points.iter().map(|p| p[axis]).collect();
        v.sort_by(f32::total_cmp);
        if v.is_empty() {
            return Vec::new();
        }
        lo[axis] = v[((v.len() - 1) as f32 * q) as usize];
        hi[axis] = v[((v.len() - 1) as f32 * (1.0 - q)) as usize];
    }
    points.iter().filter(|p| (0..3).all(|i| p[i] >= lo[i] && p[i] <= hi[i])).copied().collect()
}

/// Rotation about +y by `angle` (counter-clockwise seen from above), column-major.
pub fn rotation_y(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    [c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// Rotation about +z by `angle`, column-major.
fn rotation_z(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    [c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 × 1.3 × 4.5 m box of points, yawed by `yaw` and pitched by `pitch`.
    fn car(yaw: f32, pitch: f32) -> Vec<[f32; 3]> {
        let m = camera::mul(&rotation_y(yaw), &camera::rotation_x(pitch));
        let mut pts = Vec::new();
        for i in 0..20 {
            for j in 0..13 {
                for k in 0..45 {
                    pts.push(camera::transform_point(&m, [i as f32 * 0.1 - 1.0, j as f32 * 0.1, k as f32 * 0.1 - 2.25]));
                }
            }
        }
        pts
    }

    fn extent(points: &[[f32; 3]], axis: usize) -> f32 {
        let (lo, hi) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[axis]), hi.max(p[axis])));
        hi - lo
    }

    #[test]
    fn a_yawed_and_pitched_car_is_squared_up() {
        for (yaw, pitch) in [(0.09f32, 0.0f32), (-0.2, 0.03), (0.0, -0.04), (3.1, 0.0)] {
            let pts = car(yaw, pitch);
            let m = align(&pts);
            let aligned: Vec<[f32; 3]> = pts.iter().map(|p| camera::transform_point(&m, *p)).collect();
            assert!((extent(&aligned, 0) - 1.9).abs() < 0.08, "width {} for yaw {yaw}", extent(&aligned, 0));
            assert!((extent(&aligned, 2) - 4.4).abs() < 0.12, "length {} for yaw {yaw}", extent(&aligned, 2));
            assert!((extent(&aligned, 1) - 1.2).abs() < 0.06, "height {} for pitch {pitch}", extent(&aligned, 1));
        }
    }
}

