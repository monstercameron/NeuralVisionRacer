// CPU-only WASM diagnostic. Reference and candidate share one scalar module.
const status = document.querySelector("#status"), output = document.querySelector("#result");
const get = async path => {
  const response = await fetch(path, {cache: "no-store"});
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
};
const hash = async bytes => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), x => x.toString(16).padStart(2, "0")).join("");
function equal(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}
function codebook(section) {
  if (!section) return new Float32Array(256);
  if (!Array.isArray(section.codebook) || section.codebook.length !== 256) throw new Error("Expected 256-entry codebook");
  const book = Float32Array.from(section.codebook, v => typeof v === "number" ? v : NaN);
  if (Number.isNaN(book[0])) book[0] = Math.fround(book[1] + Math.fround(Math.fround(book[1] - book[255]) / 255));
  return book;
}
function single(api, variant, from, to) {
  const fp = api.reserve_floats(from.length), tp = api.reserve_floats(to.length), op = api.reserve(256);
  new Float32Array(api.memory.buffer, fp, from.length).set(from);
  new Float32Array(api.memory.buffer, tp, to.length).set(to);
  api.remap_one(variant, fp, from.length, tp, to.length, op);
  const result = new Uint8Array(api.memory.buffer, op, 256).slice();
  api.release(op, 256); api.release_floats(tp, to.length); api.release_floats(fp, from.length);
  return result;
}
function randomFloats(size, seed) {
  const bits = new Uint32Array(size);
  let x = seed;
  for (let i = 0; i < size; i++) { x ^= x << 13; x ^= x >>> 17; x ^= x << 5; bits[i] = x; }
  return new Float32Array(bits.buffer);
}
async function run() {
  const root = "../levels/saburi-alley-yazd/";
  const [wasm, lodBytes] = await Promise.all([get("../logs/simd-webp-probe/scalar.wasm"), get(root + "lod-meta.json")]);
  const api = (await WebAssembly.instantiate(wasm)).instance.exports;
  const paths = JSON.parse(new TextDecoder().decode(lodBytes)).filenames;
  const sources = await Promise.all(paths.map(p => get(root + p)));
  const books = sources.map(bytes => {
    const meta = JSON.parse(new TextDecoder().decode(bytes));
    return [codebook(meta.scales), codebook(meta.sh0), codebook(meta.shN)];
  });
  const chunks = books.length, count = chunks * 3 * 256;
  if (!chunks) throw new Error("No metadata fixtures");
  const bp = api.reserve_floats(count), tp = api.reserve_floats(3 * 256), mp = api.reserve_floats(chunks * 256), op = api.reserve(count);
  let bookView = new Float32Array(api.memory.buffer, bp, count);
  for (let i = 0; i < chunks; i++) for (let group = 0; group < 3; group++) bookView.set(books[i][group], (i * 3 + group) * 256);
  for (let group = 0; group < 3; group++) {
    const mergedInput = new Float32Array(api.memory.buffer, mp, chunks * 256);
    for (let i = 0; i < chunks; i++) mergedInput.set(books[i][group], i * 256);
    api.merged_book(mp, chunks * 256, tp + group * 256 * 4);
  }
  api.reserve_zero_book(tp + 2 * 256 * 4);
  api.remap_batch(0, bp, chunks, tp, 1, op);
  const expected = new Uint8Array(api.memory.buffer, op, count).slice();
  api.remap_batch(1, bp, chunks, tp, 1, op);
  if (!equal(new Uint8Array(api.memory.buffer, op, count), expected)) throw new Error("Corpus maps differ");

  const edges = [
    [[], []], [[NaN], [0]], [[-0, 0], [-0, 0, 1, 1]],
    [[-0.5, 0.5, 1.5], [-1, 0, 1, 2]],
    [[-Infinity, -4, -0, 0, 1, 1, Infinity], [-Infinity, -0, 0, 1, Infinity]],
    [[3, 1, 2], [1, 3, 2]], [[0, NaN, 1], [0, NaN, 1]],
    [Array.from({length: 300}, (_, i) => i - 150), Array.from({length: 301}, (_, i) => i - 150)],
  ];
  for (let i = 0; i < 64; i++) edges.push([randomFloats(i * 5, 0x12345678 + i), randomFloats(1 + i * 3, 0x9e3779b9 + i)]);
  const edgeOutputs = new Uint8Array(edges.length * 256);
  for (let i = 0; i < edges.length; i++) {
    const from = Float32Array.from(edges[i][0]), to = Float32Array.from(edges[i][1]);
    const a = single(api, 0, from, to), b = single(api, 1, from, to);
    if (!equal(a, b)) throw new Error(`Edge map differs: ${i}`);
    edgeOutputs.set(a, i * 256);
  }

  for (let i = 0; i < 4; i++) { api.remap_batch(0, bp, chunks, tp, 10, op); api.remap_batch(1, bp, chunks, tp, 10, op); }
  const samples = [];
  for (let round = 0; round < 20; round++) {
    const order = round % 2 ? [["candidate", 1], ["reference", 0]] : [["reference", 0], ["candidate", 1]];
    for (const [variant, mode] of order) {
      const start = performance.now();
      api.remap_batch(mode, bp, chunks, tp, 40, op);
      const ms = performance.now() - start;
      if (!equal(new Uint8Array(api.memory.buffer, op, count), expected)) throw new Error(`${variant} round ${round} differs`);
      samples.push({round, variant, repetitions: 40, ms});
    }
  }
  api.release(op, count); api.release_floats(mp, chunks * 256); api.release_floats(tp, 3 * 256); api.release_floats(bp, count);
  const result = {fixture: "all Saburi metadata codebooks", chunks, maps: chunks * 3, edgeCases: edges.length,
    moduleSha256: await hash(wasm), lodSha256: await hash(lodBytes), metaSha256: await Promise.all(sources.map(hash)),
    mapsSha256: await hash(expected), edgeMapsSha256: await hash(edgeOutputs), samples};
  output.textContent = JSON.stringify(result, null, 2);
  status.textContent = "Complete: every map matches; paired WASM timings recorded";
  window.decodeRemapProbeResult = result;
}
run().catch(e => { status.textContent = `Failed: ${e}`; throw e; });
