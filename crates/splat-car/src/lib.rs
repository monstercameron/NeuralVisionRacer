//! Car mechanics for Splat Racer: what the engine, gearbox, tyres, brakes and aero do, as forces for the physics.
//!
//! - `spec`: a car's figures, read from a `.car` file. The Porsche 911 GT3 RS (992) is built in.
//! - `car`: the running driveline. Each step takes the pedals, steering and speed, and returns the drive force at
//!   the driven wheels, the brake forces, drag, downforce and the steering angle, with telemetry (rpm, gear, power).
//!
//! Nothing here depends on the physics engine; `splat-engine` applies the forces to its rapier vehicle.

pub mod car;
pub mod spec;

pub use car::{straight_line, Car, Controls, Output};
pub use spec::{CarSpec, Drive, Layout};
