//! The agent console: one command per line in, one JSON object per line out.
//!
//! Replies are always a single JSON line, so an agent can parse stdout without guessing:
//! `{"ok":true,"cmd":"status","data":{...}}` or `{"ok":false,"cmd":"set","error":"..."}`.
//! Logs are not mixed into replies; read them with `logs` or from the log file.
//!
//! Commands:
//! - `help`                      list commands
//! - `status`                    tick, pause state, vehicle pose and speed
//! - `get <section.key>`         one setting
//! - `set <section.key> <value>` change one setting (see `dump` for the names)
//! - `dump`                      the active configuration as `engine.conf` text
//! - `reload`                    re-read the config file and rebuild the simulation
//! - `logs [count] [level]`      newest log records at or above `level`
//! - `pause`, `resume`           stop or start the fixed-step simulation
//! - `step [n]`                  advance n ticks while paused (default 1)
//! - `reset`                     put the vehicle back at `vehicle.spawn` with no velocity
//! - `flip`                      right the car where it is (anti-flip): upright, same heading, a little higher
//! - `wreck`                    development: wreck the car (it respawns after 2 s)
//! - `camera`                    show the active camera (chase or orbit)
//! - `camera reset`              back behind the car (chase), or the framing from the splat bounds (orbit)
//! - `camera follow`, `camera free`  switch to the chase camera or the free orbit camera
//! - `camera raise dy`           height: negative raises, as dragging up with the middle button
//! - `camera set ex ey ez tx ty tz`  place the eye and the target
//! - `camera orbit dx dy`, `camera pan dx dy`, `camera zoom steps`
//! - `hdri procedural` | `hdri <path>`  choose the environment (applied by the renderer)
//! - `screenshot <path.png>`    render the current frame to a PNG (works headless if a GPU exists)
//! - `entities`                  list the scene's entities with their components
//! - `entity <id>`               one entity, for example `entity 0.0`
//! - `spawn marker x y z`        make a new marker entity at a position
//! - `attach <id> spin <deg/s>`  make an entity turn about the vertical axis
//! - `attach <id> bob <amp> <hz>` make an entity bob up and down around its height
//! - `detach <id>`               remove an entity's behaviour
//! - `despawn <id>`              remove an entity (the car, floor and camera are protected)
//! - `mode practice|race|menu`  request a game mode; the reply names the transition or why it was refused
//! - `drive <throttle> <brake> <steer>`  hold the pedals (0..1) and steering (-1 left .. 1 right) until changed
//! - `shift up|down`             paddle shift
//! - `gearbox auto|manual`       PDK shifting mode
//! - `car`                       the car's spec and its live telemetry (rpm, gear, torque, power, forces)
//! - `quit`                      stop the engine

use crate::log::{json_string, Level};
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Help,
    Status,
    Get(String),
    Set(String, String),
    Dump,
    Reload,
    Logs { count: usize, level: Level },
    Pause,
    Resume,
    Step(u32),
    Reset,
    Flip,
    Wreck,
    CameraShow,
    CameraReset,
    CameraSet([f32; 6]),
    CameraOrbit(f32, f32),
    CameraPan(f32, f32),
    CameraZoom(f32),
    CameraRaise(f32),
    CameraFollow(bool),
    Hdri(String),
    Screenshot(String),
    Mode(String),
    Entities,
    Inspect(String),
    SpawnMarker([f32; 3]),
    Attach { id: String, behaviour: String, args: Vec<f32> },
    Detach(String),
    Despawn(String),
    Drive([f32; 3]),
    Shift(i8),
    Gearbox(bool),
    Car,
    Quit,
}

impl Command {
    /// The verb, for replies.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Help => "help",
            Command::Status => "status",
            Command::Get(_) => "get",
            Command::Set(..) => "set",
            Command::Dump => "dump",
            Command::Reload => "reload",
            Command::Logs { .. } => "logs",
            Command::Pause => "pause",
            Command::Resume => "resume",
            Command::Step(_) => "step",
            Command::Reset => "reset",
            Command::Flip => "flip",
            Command::Wreck => "wreck",
            Command::CameraShow | Command::CameraReset | Command::CameraSet(_) | Command::CameraOrbit(..) | Command::CameraPan(..) | Command::CameraZoom(_) | Command::CameraRaise(_) | Command::CameraFollow(_) => "camera",
            Command::Hdri(_) => "hdri",
            Command::Screenshot(_) => "screenshot",
            Command::Entities => "entities",
            Command::Inspect(_) => "entity",
            Command::SpawnMarker(_) => "spawn",
            Command::Attach { .. } => "attach",
            Command::Detach(_) => "detach",
            Command::Despawn(_) => "despawn",
            Command::Mode(_) => "mode",
            Command::Drive(_) => "drive",
            Command::Shift(_) => "shift",
            Command::Gearbox(_) => "gearbox",
            Command::Car => "car",
            Command::Quit => "quit",
        }
    }
}

/// Parses one console line. Blank lines and lines starting with `#` yield `Ok(None)`.
pub fn parse(line: &str) -> Result<Option<Command>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let verb = words[0].to_ascii_lowercase();
    let args = &words[1..];
    let floats = |items: &[&str]| -> Result<Vec<f32>, String> {
        items.iter().map(|s| s.parse::<f32>().map_err(|_| format!("`{s}` is not a number"))).collect()
    };
    let cmd = match verb.as_str() {
        "help" | "?" => Command::Help,
        "status" => Command::Status,
        "dump" => Command::Dump,
        "reload" => Command::Reload,
        "pause" => Command::Pause,
        "resume" => Command::Resume,
        "reset" => Command::Reset,
        "flip" | "unflip" | "right" => Command::Flip,
        "wreck" => Command::Wreck,
        "quit" | "exit" => Command::Quit,
        "drive" => {
            let v = floats(args)?;
            let [throttle, brake, steer] = v[..] else { return Err("usage: drive <throttle 0..1> <brake 0..1> <steer -1..1>".into()) };
            Command::Drive([throttle, brake, steer])
        }
        "shift" => match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("up") | Some("+") => Command::Shift(1),
            Some("down") | Some("-") => Command::Shift(-1),
            _ => return Err("usage: shift up|down".into()),
        },
        "gearbox" => match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("auto") => Command::Gearbox(true),
            Some("manual") => Command::Gearbox(false),
            _ => return Err("usage: gearbox auto|manual".into()),
        },
        "car" => Command::Car,
        "get" => Command::Get(one_arg(args, "get <section.key>")?),
        "set" => {
            // The value keeps its spaces, so `set physics.gravity 0 -5 0` works.
            if args.len() < 2 {
                return Err("usage: set <section.key> <value>".into());
            }
            Command::Set(args[0].to_string(), args[1..].join(" "))
        }
        "logs" => {
            let count = match args.first() {
                Some(n) => n.parse().map_err(|_| format!("`{n}` is not a count"))?,
                None => 50,
            };
            let level = match args.get(1) {
                Some(l) => l.parse()?,
                None => Level::Trace,
            };
            Command::Logs { count, level }
        }
        "step" => {
            let n = match args.first() {
                Some(n) => n.parse().map_err(|_| format!("`{n}` is not a count"))?,
                None => 1,
            };
            Command::Step(n)
        }
        "camera" => match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            None | Some("show") => Command::CameraShow,
            Some("reset") => Command::CameraReset,
            Some("set") => {
                let v = floats(&args[1..])?;
                let six: [f32; 6] = v.try_into().map_err(|_| "usage: camera set ex ey ez tx ty tz".to_string())?;
                Command::CameraSet(six)
            }
            Some("orbit") | Some("pan") => {
                let v = floats(&args[1..])?;
                let [dx, dy] = v[..] else { return Err("usage: camera orbit|pan dx dy".into()) };
                if args[0].eq_ignore_ascii_case("orbit") { Command::CameraOrbit(dx, dy) } else { Command::CameraPan(dx, dy) }
            }
            Some("zoom") => {
                let v = floats(&args[1..])?;
                let [steps] = v[..] else { return Err("usage: camera zoom steps".into()) };
                Command::CameraZoom(steps)
            }
            Some("raise") => {
                let v = floats(&args[1..])?;
                let [dy] = v[..] else { return Err("usage: camera raise dy".into()) };
                Command::CameraRaise(dy)
            }
            Some("follow") => Command::CameraFollow(true),
            Some("free") => Command::CameraFollow(false),
            Some(other) => return Err(format!("unknown camera command `{other}`")),
        },
        "hdri" => Command::Hdri(one_arg(args, "hdri procedural | hdri <path>")?),
        "screenshot" => {
            // The path is the rest of the line, so it may contain spaces.
            let path = line[verb.len()..].trim();
            if path.is_empty() {
                return Err("usage: screenshot <path.png>".into());
            }
            Command::Screenshot(path.to_string())
        }
        "mode" => Command::Mode(one_arg(args, "mode <name>")?),
        "entities" => Command::Entities,
        "entity" => Command::Inspect(one_arg(args, "entity <id>")?),
        "spawn" => {
            if args.len() != 4 || args[0] != "marker" {
                return Err("usage: spawn marker <x> <y> <z>".into());
            }
            let v = floats(&args[1..])?;
            Command::SpawnMarker([v[0], v[1], v[2]])
        }
        "attach" => {
            if args.len() < 2 {
                return Err("usage: attach <id> spin <deg/s> | attach <id> bob <amp> <hz>".into());
            }
            Command::Attach { id: args[0].to_string(), behaviour: args[1].to_string(), args: floats(&args[2..])? }
        }
        "detach" => Command::Detach(one_arg(args, "detach <id>")?),
        "despawn" => Command::Despawn(one_arg(args, "despawn <id>")?),
        other => return Err(format!("unknown command `{other}` (try `help`)")),
    };
    Ok(Some(cmd))
}

fn one_arg(args: &[&str], usage: &str) -> Result<String, String> {
    match args {
        [one] => Ok(one.to_string()),
        _ => Err(format!("usage: {usage}")),
    }
}

/// The reply to one command: whether it worked, and a JSON value (or an error message).
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub cmd: &'static str,
    pub result: Result<String, String>,
}

impl Reply {
    pub fn ok(cmd: &'static str, data_json: impl Into<String>) -> Self {
        Self { cmd, result: Ok(data_json.into()) }
    }

    pub fn err(cmd: &'static str, message: impl Into<String>) -> Self {
        Self { cmd, result: Err(message.into()) }
    }

    /// The single JSON line for stdout.
    pub fn to_json_line(&self) -> String {
        match &self.result {
            Ok(data) => format!("{{\"ok\":true,\"cmd\":\"{}\",\"data\":{data}}}", self.cmd),
            Err(message) => format!("{{\"ok\":false,\"cmd\":\"{}\",\"error\":{}}}", self.cmd, json_string(message)),
        }
    }
}

/// Starts a thread that reads lines from standard input and forwards each to `sink`, until end of input.
/// Parse errors are sent too, so the caller can reply to them in order.
pub fn spawn_stdin_reader(sink: Sender<String>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if sink.send(line).is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_commands() {
        assert_eq!(parse("status").unwrap(), Some(Command::Status));
        assert_eq!(parse("  SET  render.exposure   2.5 ").unwrap(), Some(Command::Set("render.exposure".into(), "2.5".into())));
        assert_eq!(parse("set physics.gravity 0 -5 0").unwrap(), Some(Command::Set("physics.gravity".into(), "0 -5 0".into())));
        assert_eq!(parse("logs 20 warn").unwrap(), Some(Command::Logs { count: 20, level: Level::Warn }));
        assert_eq!(parse("camera set 1 2 3 0 0 0").unwrap(), Some(Command::CameraSet([1.0, 2.0, 3.0, 0.0, 0.0, 0.0])));
        assert_eq!(parse("camera zoom -2").unwrap(), Some(Command::CameraZoom(-2.0)));
        assert_eq!(parse("camera raise -40").unwrap(), Some(Command::CameraRaise(-40.0)));
        assert_eq!(parse("camera free").unwrap(), Some(Command::CameraFollow(false)));
        assert_eq!(parse("hdri procedural").unwrap(), Some(Command::Hdri("procedural".into())));
    }

    #[test]
    fn blank_and_comment_lines_are_ignored() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("   # a note for the log").unwrap(), None);
    }

    #[test]
    fn errors_say_what_was_expected() {
        assert_eq!(parse("set").unwrap_err(), "usage: set <section.key> <value>");
        assert!(parse("frobnicate").unwrap_err().contains("unknown command `frobnicate`"));
        assert!(parse("camera set 1 2").unwrap_err().contains("usage: camera set"));
        assert!(parse("logs x").unwrap_err().contains("not a count"));
    }

    #[test]
    fn replies_are_single_json_lines() {
        assert_eq!(Reply::ok("status", r#"{"tick":3}"#).to_json_line(), r#"{"ok":true,"cmd":"status","data":{"tick":3}}"#);
        assert_eq!(Reply::err("set", "bad \"value\"").to_json_line(), r#"{"ok":false,"cmd":"set","error":"bad \"value\""}"#);
    }
}
