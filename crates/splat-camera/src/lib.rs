//! Third-person chase camera for the car.
//!
//! The camera sits behind the car's heading and looks at a point above its centroid. Position and look target are
//! damped towards their goals, so the view lags gently behind sharp turns and bumps. The heading comes from the
//! car's rotation, or from its velocity once it is moving fast enough to have a clear direction.
//!
//! Matrices are column-major, the same layout `splat-render` uses. The crate has no dependencies, so the engine and
//! tests can use it without a GPU.

pub type Vec3 = [f32; 3];
pub type Mat4 = [f32; 16];

/// The car's state as the camera needs it: centroid position, rotation quaternion `[x, y, z, w]`, and velocity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarPose {
    pub position: Vec3,
    pub rotation: [f32; 4],
    pub velocity: Vec3,
}

/// Tuning for the chase camera. Distances are metres (or splat units when the car is not scaled).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowParams {
    /// How far behind the car the eye sits.
    pub distance: f32,
    /// How far above the centroid the eye sits.
    pub height: f32,
    /// How far above the centroid the camera looks.
    pub look_height: f32,
    /// Exponential damping rate of the eye position, per second. Higher is tighter.
    pub position_damping: f32,
    /// Exponential damping rate of the look target, per second.
    pub look_damping: f32,
    /// Speed (m/s) above which velocity sets the heading instead of the body rotation.
    pub velocity_heading_speed: f32,
    /// Fixed offset between the car's local forward axis and the chase direction, in radians.
    pub heading_offset: f32,
    /// Reversing faster than `velocity_heading_speed` swings the camera round to look the way the car is going
    /// (a reverse camera). Off, the camera stays behind the nose.
    pub reverse_swing: bool,
    /// Closest and farthest the orbit pitch may swing the eye, in radians above and below the chase position.
    pub max_pitch: f32,
}

impl Default for FollowParams {
    fn default() -> Self {
        Self {
            distance: 6.0,
            height: 1.8,
            look_height: 0.8,
            position_damping: 6.0,
            look_damping: 10.0,
            velocity_heading_speed: 2.0,
            heading_offset: 0.0,
            reverse_swing: true,
            max_pitch: 1.2,
        }
    }
}

/// The follow camera. Create it from the car's pose, then call [`FollowCamera::update`] once per tick.
#[derive(Debug, Clone, PartialEq)]
pub struct FollowCamera {
    pub params: FollowParams,
    eye: Vec3,
    target: Vec3,
    /// The smoothed heading of the car, in radians around +Y (0 is +Z).
    heading: f32,
    /// Mouse-driven offsets: yaw around the car and pitch above it.
    orbit_yaw: f32,
    orbit_pitch: f32,
    /// Pan offset of the whole rig from the car, in world space. Pan moves it along the ground; raise moves it up.
    offset: Vec3,
    /// The tuning the camera started with, restored by `reset_view`.
    defaults: FollowParams,
}

impl FollowCamera {
    /// Starts the camera already in place behind `car`, so the first frame has no swoop.
    pub fn new(params: FollowParams, car: &CarPose) -> Self {
        let mut camera = Self {
            params,
            eye: [0.0; 3],
            target: [0.0; 3],
            heading: heading_of(car, &params),
            orbit_yaw: 0.0,
            orbit_pitch: 0.0,
            offset: [0.0; 3],
            defaults: params,
        };
        camera.snap(car);
        camera
    }

    /// Moves the camera to its goal for `car` immediately, with no damping. Use after a reset or respawn.
    pub fn snap(&mut self, car: &CarPose) {
        self.heading = heading_of(car, &self.params);
        self.eye = self.goal_eye(car);
        self.target = self.goal_target(car);
    }

    /// Advances the camera by `dt` seconds towards the goal for the car's current pose.
    pub fn update(&mut self, car: &CarPose, dt: f32) {
        let dt = dt.clamp(0.0, 0.25);
        let target_heading = heading_of(car, &self.params);
        self.heading += shortest_angle(self.heading, target_heading) * (1.0 - (-self.params.position_damping * dt).exp());
        let eye_goal = self.goal_eye(car);
        let look_goal = self.goal_target(car);
        let a = 1.0 - (-self.params.position_damping * dt).exp();
        let b = 1.0 - (-self.params.look_damping * dt).exp();
        self.eye = lerp(self.eye, eye_goal, a);
        self.target = lerp(self.target, look_goal, b);
    }

    /// Mouse orbit: `dx` swings the camera around the car, `dy` raises or lowers it. Pixels, as the window reports.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.orbit_yaw -= dx * 0.005;
        self.orbit_pitch = (self.orbit_pitch + dy * 0.005).clamp(-self.params.max_pitch, self.params.max_pitch);
    }

    /// Mouse pan, in pixels: moves the rig along the ground, across (`dx`) and along (`dy`) the view direction.
    /// Dragging right moves the view right; dragging down pulls the ground towards the viewer.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let h = self.heading + self.orbit_yaw;
        let (s, c) = h.sin_cos();
        let forward = [s, 0.0, c];
        let right = [c, 0.0, -s];
        let scale = self.params.distance * 0.0015;
        for i in [0, 2] {
            self.offset[i] += (-dx * right[i] + dy * forward[i]) * scale;
        }
    }

    /// Height: moves the rig up (negative `dy`, dragging up) or down, eye and look point together.
    pub fn raise(&mut self, dy: f32) {
        self.offset[1] = (self.offset[1] - dy * self.params.distance * 0.002).clamp(-50.0, 200.0);
    }

    /// Puts the rig back behind the car: no pan, no height change, no orbit, and the starting distance.
    pub fn reset_view(&mut self) {
        self.params = self.defaults;
        self.offset = [0.0; 3];
        self.orbit_yaw = 0.0;
        self.orbit_pitch = 0.0;
    }

    /// The pan and height offset from the car, in world space.
    pub fn offset(&self) -> Vec3 {
        self.offset
    }

    /// Changes the chase distance by `steps` (positive is closer), with the same feel as the orbit camera's zoom.
    pub fn zoom(&mut self, steps: f32) {
        self.params.distance = (self.params.distance * (-steps * 0.1).exp()).clamp(1.0, 200.0);
    }

    pub fn eye(&self) -> Vec3 {
        self.eye
    }

    pub fn target(&self) -> Vec3 {
        self.target
    }

    /// The world-to-view matrix, looking from `eye` at `target` with +Y up.
    pub fn view(&self) -> Mat4 {
        look_at(self.eye, self.target, [0.0, 1.0, 0.0])
    }

    fn chase_direction(&self) -> Vec3 {
        let h = self.heading + self.orbit_yaw;
        [h.sin(), 0.0, h.cos()]
    }

    fn goal_eye(&self, car: &CarPose) -> Vec3 {
        let back = self.chase_direction();
        let p = self.params;
        let lift = p.height + p.distance * self.orbit_pitch.sin();
        let horizontal = p.distance * self.orbit_pitch.cos().max(0.2);
        [
            car.position[0] - back[0] * horizontal + self.offset[0],
            car.position[1] + lift + self.offset[1],
            car.position[2] - back[2] * horizontal + self.offset[2],
        ]
    }

    fn goal_target(&self, car: &CarPose) -> Vec3 {
        [
            car.position[0] + self.offset[0],
            car.position[1] + self.params.look_height + self.offset[1],
            car.position[2] + self.offset[2],
        ]
    }
}

/// The heading the camera should face: where the car's nose points (its body's +z turned by `heading_offset`), or,
/// when moving faster than `velocity_heading_speed`, the direction of travel, so the camera swings into slides.
/// Reversing swings it round to look the way the car is going, unless `reverse_swing` is off.
pub fn heading_of(car: &CarPose, params: &FollowParams) -> f32 {
    let (s, c) = params.heading_offset.sin_cos();
    let nose = rotate(car.rotation, [s, 0.0, c]);
    let speed_xz = (car.velocity[0] * car.velocity[0] + car.velocity[2] * car.velocity[2]).sqrt();
    // Reversing: travelling within 60° of the tail. Slides, even sideways ones, still follow the velocity.
    let reversing = car.velocity[0] * nose[0] + car.velocity[2] * nose[2] < -0.5 * speed_xz;
    if speed_xz >= params.velocity_heading_speed && (params.reverse_swing || !reversing) {
        car.velocity[0].atan2(car.velocity[2])
    } else {
        nose[0].atan2(nose[2])
    }
}

/// Rotates `v` by the unit quaternion `q = [x, y, z, w]`.
pub fn rotate(q: [f32; 4], v: Vec3) -> Vec3 {
    let [x, y, z, w] = q;
    let u = [x, y, z];
    let t = cross(u, v).map(|c| c * 2.0);
    let ut = cross(u, t);
    [0, 1, 2].map(|i| v[i] + w * t[i] + ut[i])
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// The signed angle from `from` to `to`, in (−π, π].
fn shortest_angle(from: f32, to: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut d = (to - from) % tau;
    if d > std::f32::consts::PI {
        d -= tau;
    } else if d <= -std::f32::consts::PI {
        d += tau;
    }
    d
}

/// Right-handed view matrix, column-major.
pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let f = normalize([target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]]);
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        s[0], u[0], -f[0], 0.0,
        s[1], u[1], -f[1], 0.0,
        s[2], u[2], -f[2], 0.0,
        -dot(s, eye), -dot(u, eye), dot(f, eye), 1.0,
    ]
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: Vec3) -> Vec3 {
    let len = dot(v, v).sqrt().max(1e-12);
    [v[0] / len, v[1] / len, v[2] / len]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car_at(position: Vec3) -> CarPose {
        CarPose { position, rotation: [0.0, 0.0, 0.0, 1.0], velocity: [0.0; 3] }
    }

    #[test]
    fn at_rest_the_camera_sits_behind_and_above_the_car() {
        let camera = FollowCamera::new(FollowParams::default(), &car_at([10.0, 2.0, 5.0]));
        let eye = camera.eye();
        // Identity rotation faces +Z, so "behind" is -Z.
        assert!(eye[2] < 5.0 - 5.9, "eye {eye:?}");
        assert!((eye[1] - (2.0 + 1.8)).abs() < 1e-4);
        assert!((camera.target()[1] - (2.0 + 0.8)).abs() < 1e-4);
    }

    #[test]
    fn the_car_projects_to_the_centre_of_the_view() {
        let car = car_at([3.0, 1.0, -2.0]);
        let camera = FollowCamera::new(FollowParams::default(), &car);
        let view = camera.view();
        let t = camera.target();
        // View-space x and y of the look target are zero, so the target is at the centre of the viewport.
        let x = view[0] * t[0] + view[4] * t[1] + view[8] * t[2] + view[12];
        let y = view[1] * t[0] + view[5] * t[1] + view[9] * t[2] + view[13];
        assert!(x.abs() < 1e-4 && y.abs() < 1e-4, "view-space xy {x} {y}");
    }

    #[test]
    fn the_camera_follows_a_turning_car_without_a_snap() {
        let mut camera = FollowCamera::new(FollowParams::default(), &car_at([0.0, 0.0, 0.0]));
        let moved = car_at([0.0, 0.0, 20.0]);
        camera.update(&moved, 1.0 / 60.0);
        let first = camera.eye()[2];
        for _ in 0..600 {
            camera.update(&moved, 1.0 / 60.0);
        }
        assert!(camera.eye()[2] > first, "the camera should move towards the car");
        assert!((camera.eye()[2] - (20.0 - 6.0)).abs() < 0.05, "settles behind the car: {}", camera.eye()[2]);
    }

    #[test]
    fn velocity_sets_the_heading_when_moving_fast() {
        let params = FollowParams::default();
        let car = CarPose { position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], velocity: [10.0, 0.0, 0.0] };
        // Moving along +X, so the chase direction is along +X and the camera sits at negative X.
        assert!((heading_of(&car, &params) - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }

    #[test]
    fn rotation_turns_the_forward_axis() {
        // A half turn about +Y maps +Z to -Z.
        let half = [0.0, 1.0, 0.0, 0.0];
        let v = rotate(half, [0.0, 0.0, 1.0]);
        assert!((v[2] + 1.0).abs() < 1e-5, "{v:?}");
    }

    #[test]
    fn orbit_moves_the_eye_around_the_car_and_is_clamped() {
        let car = car_at([0.0; 3]);
        let mut camera = FollowCamera::new(FollowParams::default(), &car);
        let before = camera.eye();
        camera.orbit(200.0, 0.0);
        camera.update(&car, 1.0);
        let after = camera.eye();
        assert!((before[0] - after[0]).abs() + (before[2] - after[2]).abs() > 0.5);
        camera.orbit(0.0, 1e6);
        assert!(camera.orbit_pitch <= camera.params.max_pitch + 1e-6);
    }

    #[test]
    fn the_camera_sits_behind_a_car_driving_forwards_and_swings_round_when_it_reverses() {
        // Nose along -z. Forwards (vz < 0) and parked: the camera sits behind the nose, at +z.
        let params = FollowParams { heading_offset: std::f32::consts::PI, ..FollowParams::default() };
        let eye_z = |params: FollowParams, vz: f32| {
            let car = CarPose { position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], velocity: [0.0, 0.0, vz] };
            let mut camera = FollowCamera::new(params, &car);
            camera.snap(&car);
            camera.eye()[2]
        };
        assert!(eye_z(params, -20.0) > 1.0, "behind when driving forwards");
        assert!(eye_z(params, 0.0) > 1.0, "behind when parked");
        // Reversing (vz > 0): the camera swings round to look the way the car is going, so it sits at -z.
        assert!(eye_z(params, 5.0) < -1.0, "reverse camera swings round");
        assert!(eye_z(FollowParams { reverse_swing: false, ..params }, 5.0) > 1.0, "stays behind with the swing off");
    }

    #[test]
    fn pan_moves_along_the_ground_and_raise_moves_up() {
        let car = car_at([0.0; 3]);
        let mut camera = FollowCamera::new(FollowParams::default(), &car);
        let start_eye = camera.eye();
        camera.pan(100.0, 0.0);
        camera.snap(&car);
        let panned = camera.eye();
        assert!((panned[1] - start_eye[1]).abs() < 1e-5, "pan keeps the height");
        assert!((panned[0] - start_eye[0]).abs() > 0.1, "pan moves sideways: {start_eye:?} -> {panned:?}");
        camera.raise(-100.0);
        camera.snap(&car);
        assert!(camera.eye()[1] > panned[1] + 0.5, "dragging up raises the eye");
        assert!(camera.target()[1] > 0.8 + 0.5, "and the look point with it");
    }

    #[test]
    fn reset_view_returns_behind_the_car() {
        let car = car_at([1.0, 0.0, 1.0]);
        let mut camera = FollowCamera::new(FollowParams::default(), &car);
        let home = camera.eye();
        camera.pan(80.0, 40.0);
        camera.raise(-60.0);
        camera.orbit(120.0, 30.0);
        camera.zoom(3.0);
        camera.reset_view();
        camera.snap(&car);
        let back = camera.eye();
        for i in 0..3 {
            assert!((back[i] - home[i]).abs() < 1e-4, "{back:?} vs {home:?}");
        }
    }

    #[test]
    fn damping_is_frame_rate_independent_enough() {
        let car = car_at([0.0; 3]);
        let mut fast = FollowCamera::new(FollowParams::default(), &car_at([0.0; 3]));
        let mut slow = fast.clone();
        let moved = car_at([0.0, 0.0, 10.0]);
        for _ in 0..60 {
            fast.update(&moved, 1.0 / 60.0);
        }
        for _ in 0..120 {
            slow.update(&moved, 1.0 / 120.0);
        }
        assert!((fast.eye()[2] - slow.eye()[2]).abs() < 0.2);
        let _ = car;
    }
}
