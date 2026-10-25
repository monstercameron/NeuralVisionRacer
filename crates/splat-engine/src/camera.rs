//! Orbit camera state: pure data and maths, so the console can read and move it without a window.

use splat_render::camera::{self, Mat4};
use std::f32::consts::FRAC_PI_2;

/// Looks at `target` from `distance`, along `yaw` (around +Y) and `pitch` (above the horizon).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl OrbitCamera {
    /// The eye position.
    pub fn eye(&self) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let offset = [cp * sy, sp, cp * cy];
        [0, 1, 2].map(|i| self.target[i] + offset[i] * self.distance)
    }

    /// The world-to-view matrix.
    pub fn view(&self) -> Mat4 {
        camera::look_at(self.eye(), self.target, [0.0, 1.0, 0.0])
    }

    /// Orbits by a screen-space delta in pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.005;
        self.pitch = (self.pitch + dy * 0.005).clamp(-FRAC_PI_2 + 0.05, FRAC_PI_2 - 0.05);
    }

    /// Mouse pan, in pixels: moves the target along the ground, across (`dx`) and along (`dy`) the view direction,
    /// scaled to the distance so pan speed feels constant. Height is separate (`raise`).
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let (sy, cy) = self.yaw.sin_cos();
        let right = [cy, 0.0, -sy];
        let forward = [-sy, 0.0, -cy];
        let scale = self.distance * 0.0015;
        for i in [0, 2] {
            self.target[i] += (-dx * right[i] + dy * forward[i]) * scale;
        }
    }

    /// Height: moves the target, and the eye with it, up (negative `dy`, dragging up) or down.
    pub fn raise(&mut self, dy: f32) {
        self.target[1] -= dy * self.distance * 0.002;
    }

    /// Multiplies the distance by `exp(-steps × 0.1)`, so positive steps zoom in.
    pub fn zoom(&mut self, steps: f32) {
        self.distance = (self.distance * (-steps * 0.1).exp()).clamp(0.05, 1.0e4);
    }

    /// A camera framing a splat whose bounds are `[lo, hi]` in world space, from the diagonal direction the spike used.
    pub fn framing(lo: [f32; 3], hi: [f32; 3]) -> Self {
        let centre = [0, 1, 2].map(|i| (lo[i] + hi[i]) * 0.5);
        let size = [0, 1, 2].map(|i| hi[i] - lo[i]);
        let diagonal = (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
        Self {
            target: centre,
            distance: diagonal * 1.8,
            yaw: 1.0f32.atan2(1.0),
            pitch: (0.35f32 / 2.0f32.sqrt()).atan(),
        }
    }
}
