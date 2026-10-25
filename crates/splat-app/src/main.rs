//! `splat-racer`: runs the engine in a window, or headless for agents with `--headless`.
//!
//! ```text
//! splat-racer [--config engine.conf] [--headless]
//! ```
//!
//! Commands are read from standard input, one per line, and each reply is printed as one JSON line on standard
//! output (see `splat-engine`'s `console` module). Logs go to the file named by `engine.log_file`.

mod renderer;
mod window;

use renderer::Renderer;
use splat_engine::console::{self, Reply};
use splat_engine::{CarLoad, Command, Effect, Engine, EngineConfig, Level, Logger};
use splat_render::sog::{self, Bundle};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the application renders with. The bundles are kept so an offscreen renderer can be built on demand.
pub struct Frontend {
    pub bundle: Option<Bundle>,
    /// The level's splat chunks at the chosen level of detail.
    pub level: Vec<Bundle>,
    pub renderer: Option<Renderer>,
}

impl Frontend {
    /// The renderer, building an offscreen one at the configured window size if none exists yet.
    fn renderer(&mut self, engine: &Engine) -> Result<&mut Renderer, String> {
        if self.renderer.is_none() {
            let size = (engine.config.window_width, engine.config.window_height);
            self.renderer = Some(Renderer::new(None, self.bundle.as_ref(), &self.level, engine, size)?);
        }
        Ok(self.renderer.as_mut().expect("created above"))
    }

    fn reload_environment(&mut self, engine: &Engine) {
        let Some(renderer) = self.renderer.as_mut() else { return };
        match renderer.reload_environment(engine) {
            Ok(()) => engine.log.log(engine.tick, Level::Info, "renderer", "environment reloaded"),
            Err(message) => engine.log.log(engine.tick, Level::Error, "renderer", message),
        }
    }
}

/// Loads the level's splat chunks at the level of detail `level.lod` selects. Failures are logged; the level then has
/// collision but no splats.
fn load_level_splats(dir: &Path, config: &EngineConfig, log: &Logger) -> Vec<Bundle> {
    let started = Instant::now();
    let meta = match std::fs::read(dir.join("lod-meta.json")).map_err(|e| e.to_string()).and_then(|b| splat_render::lod::LodMeta::parse(&b)) {
        Ok(meta) => meta,
        Err(error) => {
            log.log(0, Level::Error, "level", format!("{}: no splats ({error})", dir.display()));
            return Vec::new();
        }
    };
    let lod = splat_engine::render::level_lod(config, &meta);
    let mut chunks = Vec::new();
    for chunk in meta.chunk_dirs(lod) {
        match sog::load_dir(&dir.join(&chunk)) {
            Ok(bundle) => chunks.push(bundle),
            Err(error) => log.log(0, Level::Error, "level", format!("chunk {chunk}: {error}")),
        }
    }
    let splats: u64 = chunks.iter().map(|c| u64::from(c.meta.count)).sum();
    log.log(0, Level::Info, "level", format!("splats lod {lod}: {} chunks, {splats} splats in {:.1?}", chunks.len(), started.elapsed()));
    chunks
}

struct Options {
    config_path: PathBuf,
    headless: bool,
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options { config_path: PathBuf::from("engine.conf"), headless: false };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => options.config_path = PathBuf::from(args.next().ok_or("--config needs a path")?),
            "--headless" => options.headless = true,
            "--help" | "-h" => {
                println!("usage: splat-racer [--config engine.conf] [--headless]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(options)
}

fn main() {
    let options = match parse_args() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("splat-racer: {message}");
            std::process::exit(2);
        }
    };
    let config = match std::fs::read_to_string(&options.config_path) {
        Ok(text) => match EngineConfig::parse(&text) {
            Ok(config) => config,
            Err(errors) => {
                for error in errors {
                    eprintln!("{}: {error}", options.config_path.display());
                }
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!("splat-racer: cannot read {}: {error}; using built-in defaults", options.config_path.display());
            EngineConfig::default()
        }
    };

    let level: Level = config.log_level.parse().unwrap_or(Level::Info);
    let log_path = (!config.log_file.is_empty()).then(|| PathBuf::from(&config.log_file));
    let log = match Logger::open(log_path.as_deref(), level, config.log_memory_lines) {
        Ok(log) => Arc::new(log),
        Err(error) => {
            eprintln!("splat-racer: cannot open log file: {error}");
            std::process::exit(1);
        }
    };
    log.log(0, Level::Info, "app", format!("starting; config {}", options.config_path.display()));

    let bundle = match config.splat_path.as_str() {
        "" => {
            log.log(0, Level::Warn, "splat", "splat.path is empty: no vehicle or environment scene");
            None
        }
        path => match sog::load(Path::new(path)) {
            Ok(bundle) => {
                log.log(0, Level::Info, "splat", format!("loaded {path}: {} splats", bundle.meta.count));
                Some(bundle)
            }
            Err(error) => {
                log.log(0, Level::Error, "splat", format!("cannot load {path}: {error}"));
                None
            }
        },
    };
    let car = match (&bundle, config.splat_path.is_empty()) {
        (Some(bundle), _) => CarLoad::Loaded(sog::centres(bundle)),
        (None, true) => CarLoad::NotConfigured,
        (None, false) => CarLoad::Failed(format!("{} could not be loaded", config.splat_path)),
    };
    let level = match config.level_path.as_str() {
        "" => None,
        path => match splat_engine::LevelFloor::load(Path::new(path), config.level_floor_clearance_m) {
            Ok(floor) => {
                log.log(0, Level::Info, "level", format!("floor from {path}: {} cells, {:.2} m per cell", floor.cells.len(), floor.resolution));
                Some(floor)
            }
            Err(error) => {
                log.log(0, Level::Error, "level", format!("cannot load {path}: {error}"));
                None
            }
        },
    };
    let engine = Engine::new(config.clone(), Some(options.config_path.clone()), log.clone(), car, level);

    let console = config.console_enabled.then(|| {
        let (tx, rx): (_, Receiver<String>) = mpsc::channel();
        if config.console_stdin {
            console::spawn_stdin_reader(tx);
        }
        rx
    });
    // Streamed levels load their chunks on demand in the renderer; otherwise one whole level of detail loads here.
    let level_splats = if config.level_path.is_empty() || config.level_stream {
        Vec::new()
    } else {
        load_level_splats(Path::new(&config.level_path), &config, &log)
    };
    let frontend = Frontend { bundle, level: level_splats, renderer: None };

    if options.headless {
        run_headless(engine, frontend, console);
    } else {
        window::run(engine, frontend, console);
    }
}

/// Runs the fixed-step loop without a window. Console commands are handled between ticks.
fn run_headless(mut engine: Engine, mut frontend: Frontend, console: Option<Receiver<String>>) {
    let step = Duration::from_secs_f64(1.0 / f64::from(engine.config.tick_hz.max(1.0)));
    let mut next = Instant::now();
    loop {
        if let Some(rx) = &console {
            while let Ok(line) = rx.try_recv() {
                if handle_console_line(&mut engine, &mut frontend, &line) == Effect::Quit {
                    return;
                }
            }
        }
        engine.tick();
        next += step;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}

/// Runs one console line, prints its reply, and applies any effect that needs the renderer.
pub fn handle_console_line(engine: &mut Engine, frontend: &mut Frontend, line: &str) -> Effect {
    match console::parse(line) {
        Ok(None) => Effect::None,
        Ok(Some(Command::Screenshot(path))) => {
            let reply = match frontend.renderer(engine).and_then(|r| r.screenshot(engine, Path::new(&path))) {
                Ok((width, height)) => {
                    engine.log.log(engine.tick, Level::Info, "renderer", format!("screenshot {path} ({width}x{height})"));
                    Reply::ok("screenshot", format!("{{\"path\":{},\"width\":{width},\"height\":{height}}}", splat_engine::log::json_string(&path)))
                }
                Err(message) => Reply::err("screenshot", message),
            };
            println!("{}", reply.to_json_line());
            Effect::None
        }
        Ok(Some(command)) => {
            let (reply, effect) = engine.execute(command);
            println!("{}", reply.to_json_line());
            if effect == Effect::ReloadEnvironment {
                frontend.reload_environment(engine);
            }
            effect
        }
        Err(message) => {
            println!("{}", Reply::err("parse", message).to_json_line());
            Effect::None
        }
    }
}
