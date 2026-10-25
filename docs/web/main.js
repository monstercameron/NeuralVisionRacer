// Browser entry: fetches the assets, starts the WebAssembly viewer on the canvas, and drives it from
// requestAnimationFrame. The engine ticks inside Viewer.frame(); this file only feeds it input and shows its state.
import init, { Viewer } from "./pkg/splat_web.js";

// Paths are relative to this page, which is served from /web/.
// The levels with collision voxels, in the order L cycles through them. The level is `[level] path` from engine.conf,
// or `?level=<folder>` in the address.
const LEVELS = ["trompetstraat-delft", "nmbu-park-03", "basilica-assumption", "saburi-alley-yazd"];

/// The levels this page can offer: those whose folders are being served. A build with one level (the GitHub Pages
/// site) offers just that one; the repository's web/ offers all of them.
async function availableLevels() {
  const present = await Promise.all(LEVELS.map((name) =>
    fetch(`../levels/${name}/lod-meta.json`, { method: "HEAD", cache: "no-store" }).then((r) => r.ok).catch(() => false)));
  return LEVELS.filter((_, i) => present[i]);
}
let LEVEL = "";
const ASSETS = {
  config: "../engine.conf",
  car: "../assets/car/porsche-992-gt3rs-2023.sog",
};

/// A value from engine.conf's text: `key` in `[section]`, without its comment; `null` when absent.
function confValue(configText, section, key) {
  const body = (configText.split(new RegExp(`^\\[${section}\\]`, "m"))[1] || "").split(/^\[/m)[0];
  const line = body.match(new RegExp(`^${key}\\s*=\\s*([^\\n]*)`, "m"));
  return line ? line[1].replace(/\s+#.*$/, "").trim() : null;
}

/// The level folder (relative to this page): `?level=` wins, then engine.conf's `[level] path` (relative to
/// the repository root; this page is one folder below it).
function levelFolder(configText) {
  const asked = new URLSearchParams(location.search).get("level");
  if (asked) return `../levels/${asked.replace(/[^\w.-]/g, "")}`;
  const section = configText.split(/^\[level\]/m)[1] || "";
  const path = (section.split(/^\[/m)[0].match(/^path\s*=\s*([^#\n]+)/m) || [])[1];
  return path ? `../${path.trim()}` : "../levels/trompetstraat-delft";
}
// A level chunk's files, in the order Viewer.add_level_chunk takes them.
const CHUNK_FILES = ["meta.json", "means_l.webp", "means_u.webp", "quats.webp", "scales.webp", "sh0.webp", "shN_labels.webp", "shN_centroids.webp"];

const canvas = document.getElementById("view");
const hud = document.getElementById("hud");
const logBox = document.getElementById("log");
const loading = document.getElementById("loading");
const form = document.getElementById("cmd");
const input = document.getElementById("line");

function say(text) {
  logBox.textContent += text + "\n";
  logBox.scrollTop = logBox.scrollHeight;
}

// Downloaded files are kept in the browser's Cache Storage (not localStorage, which holds about 5 MB and cannot take
// a 10 MB car). A cached copy is used only while the server still reports the same validator (ETag, else
// Last-Modified, else length), so a rebuilt site or edited level is fetched again.
const CACHE_NAME = "splat-racer-assets-v1";
const VALIDATOR_HEADER = "x-splat-validator";

/// Bytes of `url`, from the cache when it is current, else downloaded (and then cached). `onProgress(loaded, total,
/// fromCache)` reports as bytes arrive, for the loading screen.
async function bytes(url, onProgress = () => {}) {
  const store = typeof caches === "undefined" ? null : await caches.open(CACHE_NAME).catch(() => null);
  let validator = null;
  if (store) {
    const head = await fetch(url, { method: "HEAD", cache: "no-store" }).catch(() => null);
    if (head && head.ok) {
      validator = head.headers.get("etag") || head.headers.get("last-modified") || head.headers.get("content-length");
    }
    const cached = validator && (await store.match(url));
    if (cached && cached.headers.get(VALIDATOR_HEADER) === validator) {
      const data = new Uint8Array(await cached.arrayBuffer());
      onProgress(data.length, data.length, true);
      return data;
    }
  }
  const response = await fetch(url, { cache: "no-store" });
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  const total = Number(response.headers.get("content-length")) || 0;
  let data;
  if (response.body) {
    const reader = response.body.getReader();
    const parts = [];
    let loaded = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      parts.push(value);
      loaded += value.length;
      onProgress(loaded, total || loaded, false);
    }
    data = new Uint8Array(loaded);
    let at = 0;
    for (const part of parts) { data.set(part, at); at += part.length; }
  } else {
    data = new Uint8Array(await response.arrayBuffer());
    onProgress(data.length, data.length, false);
  }
  if (store && validator) {
    // Quota or private-browsing failures only mean the next visit downloads again.
    const headers = { [VALIDATOR_HEADER]: validator, "content-type": response.headers.get("content-type") || "application/octet-stream" };
    store.put(url, new Response(data, { headers })).catch(() => {});
  }
  return data;
}

/// Empties the asset cache (for the console: `splatClearCache()`).
window.splatClearCache = () => (typeof caches === "undefined" ? Promise.resolve(false) : caches.delete(CACHE_NAME));

/// The loading screen: one bar for the files it is waiting on, with the current file and how much came from cache.
const progress = {
  files: new Map(),
  label(text) {
    document.getElementById("loading-label").textContent = text;
  },
  track(name) {
    const entry = { loaded: 0, total: 0, cached: false, done: false };
    this.files.set(name, entry);
    return (loaded, total, fromCache) => {
      Object.assign(entry, { loaded, total, cached: fromCache });
      this.draw(name);
    };
  },
  finish(name) {
    const entry = this.files.get(name);
    if (entry) entry.done = true;
    this.draw(name);
  },
  draw(current) {
    const all = [...this.files.values()];
    const total = all.reduce((sum, f) => sum + (f.total || 0), 0);
    const loaded = all.reduce((sum, f) => sum + (f.done ? (f.total || f.loaded) : f.loaded), 0);
    const share = total > 0 ? Math.min(1, loaded / total) : all.filter((f) => f.done).length / Math.max(1, all.length);
    document.getElementById("loading-fill").style.width = `${Math.round(share * 100)}%`;
    const mb = (n) => (n / 1e6).toFixed(1);
    const cached = all.filter((f) => f.done && f.cached).length;
    const entry = this.files.get(current);
    const where = entry ? (entry.cached ? " (cached)" : "") : "";
    document.getElementById("loading-detail").textContent =
      `${mb(loaded)} / ${mb(total)} MB · ${all.filter((f) => f.done).length}/${all.length} files` +
      (cached ? ` · ${cached} from cache` : "") + where;
  },
};

function fitCanvas() {
  const ratio = window.devicePixelRatio || 1;
  const width = Math.round(canvas.clientWidth * ratio);
  const height = Math.round(canvas.clientHeight * ratio);
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
    return true;
  }
  return false;
}

// The engine renders with WebGPU and has no WebGL fallback (it needs compute and storage buffers), so check for a
// GPU adapter before fetching anything, and explain the problem in the page rather than showing a raw error.
async function requireWebGpu() {
  // Browsers only expose WebGPU on secure pages. Plain http on a network address (http://192.168.x.x) is not secure;
  // http://localhost is. The bundled server redirects this Mac's own requests to localhost, so this mostly catches
  // other machines on the network.
  if (!window.isSecureContext) {
    const local = `${location.protocol}//localhost${location.port ? ":" + location.port : ""}${location.pathname}`;
    // On the machine running the server, move to localhost, which is secure. The server answers from the caller's address.
    try {
      const answer = await (await fetch("/__splat/is-local", { cache: "no-store" })).json();
      if (answer.local) {
        location.replace(local);
        return new Promise(() => {});
      }
    } catch (_) {
      // Another server, or no answer: fall through to the explanation.
    }
    const error = new Error(
      "This page was opened at " + location.origin + ", which the browser does not treat as secure, so it turns WebGPU off.\n" +
      "On the computer running the server, open the localhost address instead. Other computers need the page served over HTTPS.");
    error.link = local;
    throw error;
  }
  if (!navigator.gpu) {
    throw new Error(
      "This browser has no WebGPU, which Splat Racer needs.\n" +
      "Use a recent Chrome or Edge on a computer with a GPU, or enable WebGPU in your browser's settings.");
  }
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) {
    throw new Error(
      "WebGPU is present but found no GPU adapter.\n" +
      "Check that hardware acceleration is on in your browser settings and that the GPU is not blocked (chrome://gpu in Chrome).");
  }
  return adapter;
}

async function main() {
  await requireWebGpu();
  await init();
  const configText = await fetch(ASSETS.config).then((r) => r.text());
  LEVEL = levelFolder(configText);
  ASSETS.levelJson = `${LEVEL}/scene.voxel.json`;
  ASSETS.levelBin = `${LEVEL}/scene.voxel.bin`;
  ASSETS.levelLods = `${LEVEL}/lod-meta.json`;
  // The files the engine needs before the first frame, each with its share of the loading screen.
  const start = (name, url, optional = false) => {
    progress.label(`Loading ${name}…`);
    const report = progress.track(name);
    return bytes(url, report)
      .then((data) => { progress.finish(name); return data; })
      .catch((error) => { if (optional) { progress.finish(name); return new Uint8Array(0); } throw error; });
  };
  const [car, levelJson, levelBin, viewerSettings] = await Promise.all([
    start("the car model", ASSETS.car),
    start("the level", ASSETS.levelJson),
    start("the level's collision", ASSETS.levelBin),
    // The scan's starting camera, used to find an open spawn; optional.
    start("the camera", `${LEVEL}/viewer-settings.json`, true),
  ]);
  progress.label("Starting the engine…");
  document.title = `Splat Racer · ${LEVEL.split("/").pop()}`;
  fitCanvas();
  const viewer = await Viewer.create("view", configText, car, levelJson, levelBin, viewerSettings);
  // The car spec and HUD design files named in engine.conf (paths relative to the repository root), so editing them needs
  // only a reload. A built-in HUD name needs no file.
  const specPath = confValue(configText, "vehicle", "spec");
  if (specPath) {
    try { viewer.set_car_spec(await fetch(`../${specPath}`, { cache: "no-store" }).then((r) => r.text())); }
    catch (error) { say(`car spec ${specPath}: ${error.message || error}`); }
  }
  const hudPath = confValue(configText, "hud", "design");
  if (hudPath && hudPath.endsWith(".hud")) {
    try { viewer.set_hud(await fetch(`../${hudPath}`, { cache: "no-store" }).then((r) => r.text())); }
    catch (error) { say(`hud design ${hudPath}: ${error.message || error}`); }
  }
  loading.remove();
  say("ready. The engine ticks in the browser; type `help` below.");

  let frames = 0;
  let lastHud = 0;
  // Stopping cancels the animation loop outright: no ticks, no sorting, no drawing, so the GPU and CPU go idle.
  // The simulation is paused for the stop (if it was running), so it does not jump forward on resume.
  let rafId = 0;
  let stopped = false;
  let pausedByStop = false;
  const stopButton = document.getElementById("stop");
  const stoppedNote = document.getElementById("stopped");
  function setStopped(value) {
    if (value === stopped) return;
    stopped = value;
    if (stopped) {
      cancelAnimationFrame(rafId);
      pausedByStop = !JSON.parse(viewer.status()).paused;
      if (pausedByStop) viewer.command("pause");
    } else {
      if (pausedByStop) viewer.command("resume");
      pausedByStop = false;
      rafId = requestAnimationFrame(animate);
    }
    stopButton.textContent = stopped ? "▶ Resume rendering" : "⏸ Stop rendering";
    stopButton.setAttribute("aria-pressed", String(stopped));
    stoppedNote.style.display = stopped ? "block" : "none";
  }
  stopButton.addEventListener("click", () => setStopped(!stopped));
  // Driving: W/↑ throttle, S/↓ brake (held at a standstill: reverse), A/← D/→ steer, Q/E paddles, G auto/manual.
  // Keys are on/off, so the steering eases towards full lock and back rather than jumping.
  const held = new Set();
  let steer = 0;
  let lastFrame = 0;
  const DRIVE_KEYS = ["KeyW", "KeyS", "KeyA", "KeyD", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"];
  function drive(now) {
    const dt = lastFrame ? Math.min((now - lastFrame) / 1000, 0.1) : 0;
    lastFrame = now;
    const has = (...codes) => codes.some((c) => held.has(c));
    const target = (has("KeyD", "ArrowRight") ? 1 : 0) - (has("KeyA", "ArrowLeft") ? 1 : 0);
    const rate = target === 0 ? 4 : 2.5; // lock per second: back to centre faster than out to lock
    steer += Math.max(-rate * dt, Math.min(rate * dt, target - steer));
    viewer.set_controls(has("KeyW", "ArrowUp") ? 1 : 0, has("KeyS", "ArrowDown") ? 1 : 0, steer);
  }
  window.addEventListener("keyup", (e) => held.delete(e.code));
  window.addEventListener("blur", () => held.clear());

  function animate(now) {
    if (fitCanvas()) viewer.resize(canvas.width, canvas.height);
    // Frame pacing: render only on the refreshes the pacer picks, so every frame is shown equally long.
    if (!viewer.should_render(now)) {
      rafId = requestAnimationFrame(animate);
      return;
    }
    drive(now);
    viewer.frame(now);
    frames += 1;
    if (now - lastHud > 250) {
      const status = JSON.parse(viewer.status());
      const v = status.vehicle;
      const fps = frames / ((now - lastHud) / 1000 || 1);
      frames = 0;
      lastHud = now;
      hud.textContent =
        `mode ${status.game.mode}   tick ${status.tick}   ${status.paused ? "PAUSED" : "running"}   ${fps.toFixed(0)} fps` +
        (() => { const [refresh, span, gpu] = viewer.pacing(); return `  (paced: ${(1000 / (refresh * span)).toFixed(0)} Hz, GPU ${gpu.toFixed(1)} ms)\n`; })() +
        (v ? `car ${v.state}  contacts ${v.ground_contacts}` : "no car") +
        (status.car ? `\n${status.car.speed_kmh.toFixed(0).padStart(3)} km/h  gear ${status.car.gear} (${status.car.gearbox})  ${status.car.rpm.toFixed(0).padStart(4)} rpm` +
          `  ${status.car.torque_nm} Nm  ${status.car.power_kw} kW` +
          `${status.car.launch ? "  LAUNCH" : ""}${status.car.wheelspin ? "  WHEELSPIN" : ""}${status.car.limiter ? "  LIMITER" : ""}` : "") +
        "\nW/S throttle·brake  A/D steer  Q/E shift  G auto/manual  T flip upright" +
        "\nB LOD boxes: 0 green · 1 yellow · 2 orange · 3 red (bright: loading)" +
        "\nC colliders: floor cyan · walls magenta   H dashboard" +
        `\ncamera ${viewer.camera_mode()}   F follow/free   R reset   P stop   L next level   Esc menu` +
        "\nleft rotate · right/shift pan\nmiddle/option height · wheel zoom";
    }
    rafId = requestAnimationFrame(animate);
  }
  rafId = requestAnimationFrame(animate);

  // The level's splats load while the game already runs (its collision is loaded above). With `level.stream` the
  // viewer asks for chunk folders as the camera moves (LOD 0 near, coarser far); otherwise one level of detail loads.
  if (viewer.streaming()) {
    startStreaming(viewer).catch((error) => say("level streaming failed: " + (error.message || error)));
  } else {
    streamLevel(viewer).catch((error) => say("level splats failed: " + (error.message || error)));
  }

  // Mouse: left drag rotates, right drag (or shift + left) pans along the ground, middle drag (or option/alt + left)
  // sets the camera height, and the wheel zooms. The tool is picked when the button goes down and kept for the drag.
  let drag = null;
  function toolFor(e) {
    if (e.button === 1 || (e.button === 0 && e.altKey)) return "raise";
    if (e.button === 2 || (e.button === 0 && e.shiftKey)) return "pan";
    return e.button === 0 ? "orbit" : null;
  }
  canvas.addEventListener("pointerdown", (e) => {
    const tool = toolFor(e);
    if (!tool) return;
    e.preventDefault(); // a middle press would otherwise start the browser's autoscroll
    drag = { x: e.clientX, y: e.clientY, tool, button: e.button };
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener("mousedown", (e) => { if (e.button === 1) e.preventDefault(); });
  canvas.addEventListener("auxclick", (e) => e.preventDefault());
  canvas.addEventListener("pointermove", (e) => {
    if (!drag) return;
    const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
    drag.x = e.clientX; drag.y = e.clientY;
    if (drag.tool === "raise") viewer.raise(dy);
    else if (drag.tool === "pan") viewer.pan(dx, dy);
    else viewer.orbit(dx, dy);
  });
  const endDrag = (e) => { if (drag && e.button === drag.button) drag = null; };
  canvas.addEventListener("pointerup", endDrag);
  canvas.addEventListener("pointercancel", () => { drag = null; });
  canvas.addEventListener("contextmenu", (e) => e.preventDefault());
  // Proportional to the scroll distance, so a trackpad's many small events zoom smoothly and a wheel notch is one step.
  canvas.addEventListener("wheel", (e) => {
    e.preventDefault();
    const pixels = e.deltaMode === 1 ? e.deltaY * 33 : e.deltaMode === 2 ? e.deltaY * 400 : e.deltaY;
    viewer.zoom(Math.max(-3, Math.min(3, -pixels / 100)));
  }, { passive: false });

  // Pause menu (Esc): pauses the simulation and offers the levels. Resuming only un-pauses what the menu paused.
  const menu = document.getElementById("menu");
  const available = await availableLevels();
  const menuLevels = document.getElementById("menu-levels");
  const here = LEVEL.split("/").pop();
  let menuPaused = false;
  for (const level of available) {
    const button = document.createElement("button");
    button.textContent = level;
    button.setAttribute("aria-current", String(level === here));
    button.addEventListener("click", () => {
      if (level === here) closeMenu();
      else location.search = `?level=${level}`;
    });
    menuLevels.appendChild(button);
  }
  document.getElementById("menu-resume").addEventListener("click", () => closeMenu());
  function openMenu() {
    if (!menu.hidden) return;
    held.clear();
    menuPaused = !JSON.parse(viewer.status()).paused;
    if (menuPaused) viewer.command("pause");
    menu.hidden = false;
    (menuLevels.querySelector('[aria-current="true"]') || menuLevels.firstChild)?.focus();
  }
  function closeMenu() {
    if (menu.hidden) return;
    menu.hidden = true;
    if (menuPaused) viewer.command("resume");
    menuPaused = false;
    canvas.focus?.();
  }

  window.addEventListener("keydown", (e) => {
    if (e.target === input) return;
    if (e.key === "Escape") {
      e.preventDefault();
      if (menu.hidden) openMenu(); else closeMenu();
      return;
    }
    if (!menu.hidden) {
      // Inside the menu: arrows move between the buttons; Enter and Space press them (the browser does that).
      const buttons = [...menu.querySelectorAll("button")];
      const at = buttons.indexOf(document.activeElement);
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const step = e.key === "ArrowDown" ? 1 : -1;
        buttons[(at + step + buttons.length) % buttons.length]?.focus();
      }
      return;
    }
    if (e.code === "Space") { e.preventDefault(); say(viewer.command(JSON.parse(viewer.status()).paused ? "resume" : "pause")); }
    if (e.key === "r") say(viewer.command("camera reset"));
    if (e.key === "f") say(viewer.toggle_follow());
    if (e.code === "KeyT" && !e.repeat) say(viewer.command("flip"));
    // Debug views: B the level's LOD octree, C the voxel collision.
    const toggle = (key) => {
      const on = JSON.parse(viewer.command(`get ${key}`)).data?.value === "true";
      say(viewer.command(`set ${key} ${!on}`));
    };
    if (e.code === "KeyB" && !e.repeat) toggle("render.debug_lod_boxes");
    if (e.code === "KeyC" && !e.repeat) toggle("render.debug_colliders");
    if (e.code === "KeyH" && !e.repeat) toggle("hud.enabled");
    if (e.key === "p") setStopped(!stopped);
    if (e.key === "l") {
      // Next level: reload with ?level= set to the one after this.
      const here = available.indexOf(LEVEL.split("/").pop());
      location.search = `?level=${available[(here + 1) % available.length]}`;
    }
    if (DRIVE_KEYS.includes(e.code)) { e.preventDefault(); held.add(e.code); }
    if (e.code === "KeyE" && !e.repeat) viewer.shift(1);
    if (e.code === "KeyQ" && !e.repeat) viewer.shift(-1);
    if (e.code === "KeyG" && !e.repeat) say(viewer.command(JSON.parse(viewer.status()).car?.gearbox === "auto" ? "gearbox manual" : "gearbox auto"));
  });

  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const line = input.value.trim();
    if (!line) return;
    say("> " + line);
    say(viewer.command(line));
    input.value = "";
  });
  window.splat = viewer; // for the browser console and the verification tools
}

// Streaming: every chunk's small meta.json up front (shared ranges and codebooks), then chunk folders on request.
async function startStreaming(viewer) {
  const lodMeta = await bytes(ASSETS.levelLods);
  const filenames = JSON.parse(new TextDecoder().decode(lodMeta)).filenames;
  const metas = await Promise.all(filenames.map((f) => bytes(`${LEVEL}/${f}`).then((b) => new TextDecoder().decode(b)).catch(() => "null")));
  viewer.start_stream(lodMeta, new TextEncoder().encode(`[${metas.join(",")}]`));
  say(`streaming the level: ${filenames.length} chunk files, fetched as the camera needs them`);
  const fetchChunk = async (file, dir) => {
    try {
      // Scans saved without spherical harmonics have no shN files; their meta.json says so.
      const meta = await bytes(`${LEVEL}/${dir}/meta.json`);
      const hasSh = "shN" in JSON.parse(new TextDecoder().decode(meta));
      const rest = await Promise.all(CHUNK_FILES.slice(1).map((name) => (hasSh || !name.startsWith("shN") ? bytes(`${LEVEL}/${dir}/${name}`) : new Uint8Array(0))));
      viewer.add_stream_chunk(file, meta, ...rest);
    } catch (error) {
      viewer.level_chunk_failed(file);
      console.warn("chunk", dir, error);
    }
  };
  // Poll for requests a few times a second; the viewer limits how many are in flight.
  setInterval(() => {
    for (const [file, dir] of JSON.parse(viewer.level_requests())) fetchChunk(file, dir);
  }, 100);
  window.splatStream = () => JSON.parse(viewer.stream_stats());
}

async function streamLevel(viewer) {
  const dirs = JSON.parse(viewer.level_chunk_dirs(await bytes(ASSETS.levelLods)));
  let splats = 0;
  // Fetch the next chunk while the current one uploads.
  const fetchChunk = (dir) => Promise.all(CHUNK_FILES.map((file) => bytes(`${LEVEL}/${dir}/${file}`)));
  let pending = dirs.length ? fetchChunk(dirs[0]) : null;
  for (let i = 0; i < dirs.length; i++) {
    const files = await pending;
    pending = i + 1 < dirs.length ? fetchChunk(dirs[i + 1]) : null;
    splats += viewer.add_level_chunk(...files);
    say(`level chunk ${dirs[i]} (${i + 1}/${dirs.length}), ${(splats / 1e6).toFixed(2)} M splats`);
    await new Promise((resolve) => setTimeout(resolve, 0)); // let a frame draw between chunks
  }
}

main().catch((error) => {
  loading.style.whiteSpace = "pre-wrap";
  loading.style.textAlign = "left";
  loading.textContent = "Splat Racer could not start.\n\n" + (error.message || error);
  if (error.link) {
    const anchor = document.createElement("a");
    anchor.href = error.link;
    anchor.textContent = error.link;
    anchor.style.color = "#8ab4ff";
    loading.append("\n\n", anchor);
  }
  console.error(error);
});
