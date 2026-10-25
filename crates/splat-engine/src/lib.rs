//! Engine services for Splat Racer: `engine.conf`, the agent console and logs, voxel colliders, and the rapier
//! physics world. Rendering lives in `splat-render`; the windowed application lives in `splat-app`.

pub mod camera;
pub mod collision;
pub mod car_fit;
pub mod config;
pub mod console;
pub mod engine;
pub mod floor;
pub mod level;
pub mod log;
pub mod pacing;
pub mod physics;
pub mod drive;
pub mod wheels;
pub mod render;
pub mod scene;
pub mod svo;
pub mod voxel;

pub use camera::OrbitCamera;
pub use level::LevelFloor;
pub use config::{ConfigError, EngineConfig};
pub use console::{Command, Reply};
pub use engine::{CarLoad, Effect, Engine};
pub use log::{Level, Logger};
pub use physics::{BodyPose, Physics, VehicleBody};
