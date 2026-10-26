// Deliberately uses the production WASM viewer, GPU timestamp profiler and assets.
// Fixed LOD, dimensions and paused physics isolate renderer changes from streaming churn.
const params = new URLSearchParams(location.search);
const build = params.get("build") || "current";
if (!["baseline", "current"].includes(build)) throw new Error("Invalid build");
const modulePath = build === "baseline" ? "../logs/browser-benchmark/baseline-pkg/splat_web.js" : "./pkg/splat_web.js";
const {default:init, Viewer} = await import(modulePath);
const level = params.get("level") || "saburi-alley-yazd";
if (!/^[\w-]+$/.test(level)) throw new Error("Invalid level name");
const frames = Math.max(30, Math.min(600, Math.trunc(Number(params.get("frames"))) || 120));
const label = params.get("label") || "unlabelled";
const status = document.getElementById("status");
const result = document.getElementById("result");
const data = async (path, optional = false) => {
  const response = await fetch(path, { cache: "no-store" });
  if (!response.ok) { if (optional) return new Uint8Array(); throw new Error(`${path}: ${response.status}`); }
  return new Uint8Array(await response.arrayBuffer());
};
function setting(text, section, key, value) {
  const start = text.indexOf(`[${section}]`);
  if (start < 0) throw new Error(`Missing section ${section}`);
  const end = text.indexOf("\n[", start + 1);
  const part = text.slice(start, end < 0 ? undefined : end);
  const pattern = new RegExp(`^${key}\\s*=.*$`, "m");
  if (!pattern.test(part)) throw new Error(`Missing ${section}.${key}`);
  return text.slice(0, start) + part.replace(pattern, `${key} = ${value}`) + (end < 0 ? "" : text.slice(end));
}
const raf = () => new Promise(resolve => requestAnimationFrame(resolve));
async function run() {
  await init();
  const wasmPath = new URL("splat_web_bg.wasm",new URL(modulePath,location.href));
  const wasm = await (await fetch(wasmPath,{cache:"no-store"})).arrayBuffer();
  const wasm_sha256 = [...new Uint8Array(await crypto.subtle.digest("SHA-256",wasm))].map(b=>b.toString(16).padStart(2,"0")).join("");
  let config = await (await fetch("../engine.conf", {cache: "no-store"})).text();
  for (const [section, key, value] of [["level","stream","false"],["level","lod","1"],
    ["render","debug_colliders","false"],["render","debug_lod_boxes","false"],
    ["render","frame_pacing","off"],["effects","grain","0"]]) config = setting(config,section,key,value);
  const dir = `../levels/${level}`;
  const [car, voxels, binary, settings, lod] = await Promise.all([
    data("../assets/car/porsche-992-gt3rs-2023.sog"), data(`${dir}/scene.voxel.json`),
    data(`${dir}/scene.voxel.bin`), data(`${dir}/viewer-settings.json`,true), data(`${dir}/lod-meta.json`)]);
  const viewer = await Viewer.create("view",config,car,voxels,binary,settings,`levels/${level}`);
  // Match the game's external car spec and HUD rather than relying on built-ins.
  const value = (section,key) => {
    const part=(config.split(new RegExp(`^\\[${section}\\]`,"m"))[1] || "").split(/^\[/m)[0];
    return part.match(new RegExp(`^${key}\\s*=\\s*([^#\\n]+)`,"m"))?.[1].trim();
  };
  const spec=value("vehicle","spec");
  if(spec) viewer.set_car_spec(await (await fetch(`../${spec}`,{cache:"no-store"})).text());
  const hud=value("hud","design");
  if(hud) viewer.set_hud(hud.endsWith(".hud") ? await (await fetch(`../${hud}`,{cache:"no-store"})).text() : hud);
  const command = line => { const reply = JSON.parse(viewer.command(line)); if (!reply.ok) throw new Error(`${line}: ${reply.error}`); return reply.data; };
  const presentation = params.get("presentation");
  if (presentation) {
    if (!["direct","copy"].includes(presentation)) throw new Error("Invalid presentation path");
    command(`perf present ${presentation}`);
  }
  command("perf log off"); command("pause"); command("step 180");
  const files = ["meta.json","means_l.webp","means_u.webp","quats.webp","scales.webp","sh0.webp","shN_labels.webp","shN_centroids.webp"];
  const dirs = JSON.parse(viewer.level_chunk_dirs(lod));
  for (let i = 0; i < dirs.length; i++) {
    status.textContent = `Loading fixed LOD 1: ${i+1}/${dirs.length} chunks`;
    const parts = await Promise.all(files.map(name=>data(`${dir}/${dirs[i]}/${name}`)));
    viewer.add_level_chunk(...parts);
    await raf();
  }
  const camera = params.get("camera");
  if (camera) {
    const values=camera.split(",").map(Number);
    if (values.length!==6 || !values.every(Number.isFinite)) throw new Error("Camera needs six finite coordinates");
    command(`camera set ${values.join(" ")}`);
  }
  command("perf sample 1");
  for (let i=0; i<40;) { const now=await raf(); if (viewer.should_render(now)) {viewer.frame(now);i++;} }
  command("perf reset");
  const cpu = [], intervals = [];
  let previous = null;
  status.textContent = `Measuring ${label}: ${frames} frames; 1280×720; paused physics; fixed LOD 1`;
  for (let i=0; i<frames;) {
    const now = await raf();
    if (!viewer.should_render(now)) continue;
    i++;
    if (previous !== null) intervals.push(now-previous);
    previous = now;
    const start = performance.now(); viewer.frame(now); cpu.push(performance.now()-start);
  }
  // Let asynchronous timestamp readbacks finish without submitting additional frames.
  for (let i=0; i<8; i++) await raf();
  const profile = command("perf");
  const verification = [];
  if (params.get("verify") === "1") {
    for (const [width,height] of [[960,540],[1280,720]]) {
      const canvas=document.getElementById("view");canvas.width=width;canvas.height=height;
      viewer.resize(width,height);
      for (const enabled of [true,false]) {
        command(`set render.debug_colliders ${enabled}`);
        command(`set render.debug_lod_boxes ${enabled}`);
        for (let i=0;i<4;) { const now=await raf();if(viewer.should_render(now)){viewer.frame(now);i++;} }
        verification.push({width,height,debug:enabled});
      }
    }
  }
  // Capture in the submitting task so the WebGPU canvas has not been recycled.
  viewer.frame(await raf());
  const image = document.createElement("a"); image.id="reference-image"; image.download=`${label}.png`;
  image.href=document.getElementById("view").toDataURL("image/png"); image.textContent="Download reference image";
  result.before(image);
  const report = {label, build, wasm_sha256, verification, level, dimensions:[1280,720], fixed_lod:1, frames, chunks:dirs.length,
    timestamp:new Date().toISOString(), user_agent:navigator.userAgent, cpu_submission_ms:cpu,
    raf_intervals_ms:intervals, profile};
  result.textContent = JSON.stringify(report,null,2);
  result.dataset.complete = "true";
  status.textContent = `Complete: ${label}. Renderer is idle. Results below; scene remains visible.`;
}
run().catch(error=>{status.textContent=`Benchmark failed: ${error.message || error}`; result.textContent=error.stack || String(error); result.dataset.error="true";});
