// Kernel-only diagnostic: both modules are separate from the game build.
const status = document.querySelector("#status"), output = document.querySelector("#result");
const get = async path => {
  const r = await fetch(path, {cache: "no-store"});
  if (!r.ok) throw new Error(`${path}: ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
};
const hash = async bytes => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), x => x.toString(16).padStart(2, "0")).join("");
function dimensions(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (bytes.length < 12 || String.fromCharCode(...bytes.slice(0, 4)) !== "RIFF" || String.fromCharCode(...bytes.slice(8, 12)) !== "WEBP") throw new Error("Expected RIFF WebP");
  for (let pos = 12; pos + 8 <= bytes.length;) {
    const tag = String.fromCharCode(...bytes.slice(pos, pos + 4)), size = view.getUint32(pos + 4, true);
    if (pos + 8 + size > bytes.length) throw new Error("Truncated WebP chunk");
    if (tag === "VP8L" && size >= 5 && bytes[pos + 8] === 0x2f) {
      const bits = view.getUint32(pos + 9, true);
      return [(bits & 0x3fff) + 1, ((bits >>> 14) & 0x3fff) + 1];
    }
    pos += 8 + size + (size & 1);
  }
  throw new Error("No VP8L dimensions found");
}
function input(size) {
  const bytes = new Uint8Array(size);
  let x = 0x9e3779b9;
  for (let i = 0; i < size; i++) { x ^= x << 13; x ^= x >>> 17; x ^= x << 5; bytes[i] = x; }
  return bytes;
}
function expected(bytes) {
  const copy = bytes.slice();
  for (let i = 0; i + 3 < copy.length; i += 4) copy[i + 2] = 0;
  return copy;
}
function invoke(module, bytes, iterations = 1) {
  const m = module.instance.exports, ptr = m.reserve(bytes.length);
  let view = new Uint8Array(m.memory.buffer, ptr, bytes.length);
  view.set(bytes);
  const start = performance.now();
  for (let i = 0; i < iterations; i++) m.normalize(ptr, bytes.length);
  const ms = performance.now() - start;
  view = new Uint8Array(m.memory.buffer, ptr, bytes.length);
  const result = view.slice();
  m.release(ptr, bytes.length);
  return {ms, result};
}
function equal(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}
async function run() {
  const [scalarBytes, simdBytes, labelWebp] = await Promise.all([
    get("../logs/simd-decode-labels/scalar.wasm"), get("../logs/simd-decode-labels/simd.wasm"),
    get("../levels/saburi-alley-yazd/1_0/shN_labels.webp"),
  ]);
  const scalar = await WebAssembly.instantiate(scalarBytes);
  let simd, unsupported = null;
  try { simd = await WebAssembly.instantiate(simdBytes); } catch (e) { unsupported = String(e); }
  const [width, height] = dimensions(labelWebp), labelBytes = width * height * 4;
  const sizes = [...Array.from({length: 34}, (_, i) => i), 63, 64, 65, 255, 256, 257, labelBytes];
  const cases = [];
  for (const size of sizes) {
    const source = input(size), want = expected(source), a = invoke(scalar, source).result;
    const b = simd ? invoke(simd, source).result : null;
    if (!equal(a, want) || (b && !equal(b, want))) throw new Error(`Mismatch for ${size} bytes`);
    cases.push({size, sha256: await hash(want)});
  }
  const source = input(labelBytes), samples = [];
  if (simd) {
    // Alternate order to reduce drift. Each sample normalizes the same full-size
    // buffer repeatedly; only the Rust/WASM loop is inside the timer.
    for (let i = 0; i < 4; i++) { invoke(scalar, source, 5); invoke(simd, source, 5); }
    for (let round = 0; round < 20; round++) {
      const order = round % 2 ? [["simd", simd], ["scalar", scalar]] : [["scalar", scalar], ["simd", simd]];
      for (const [variant, module] of order) samples.push({round, variant, iterations: 20, ms: invoke(module, source, 20).ms});
    }
  }
  const result = {fixture: "synthetic bytes, dimensions from Saburi 1_0 labels", width, height, labelBytes,
    scalarModuleSha256: await hash(scalarBytes), simdModuleSha256: await hash(simdBytes), sourceWebpSha256: await hash(labelWebp),
    unsupported, cases, samples};
  output.textContent = JSON.stringify(result, null, 2);
  status.textContent = simd ? "Complete: byte-identical; kernel-only timings" : "SIMD unsupported; scalar validation complete";
  window.simdDecodeLabelsResult = result;
}
run().catch(e => { status.textContent = `Failed: ${e}`; throw e; });
