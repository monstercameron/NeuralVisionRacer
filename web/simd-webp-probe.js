// Two independently compiled standalone modules use the actual SOG decoder.
// No game module, GPU or persistent state is touched.
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
function invoke(module, stream, input, expected = null) {
  const api = module.instance.exports, inputPtr = api.reserve(input.length);
  new Uint8Array(api.memory.buffer, inputPtr, input.length).set(input);
  const start = performance.now();
  const resultPtr = api.decode(stream, inputPtr, input.length);
  const ms = performance.now() - start;
  const succeeded = !!api.succeeded(resultPtr), width = api.width(resultPtr), height = api.height(resultPtr);
  const pixelPtr = api.pixels(resultPtr), pixelLength = api.pixels_len(resultPtr);
  const errorPtr = api.error(resultPtr), errorLength = api.error_len(resultPtr);
  const pixelsView = pixelLength ? new Uint8Array(api.memory.buffer, pixelPtr, pixelLength) : new Uint8Array();
  // For repeated timed samples, compare the live WASM output directly and do
  // not allocate a multi-megabyte JS copy between the two timed variants.
  const pixels = expected ? pixelsView : pixelsView.slice();
  const error = errorLength ? new TextDecoder().decode(new Uint8Array(api.memory.buffer, errorPtr, errorLength)) : "";
  const result = {ms, succeeded, width, height, pixels, error};
  if (expected) check(result, expected, `stream ${stream}`);
  api.release_outcome(resultPtr);
  api.release(inputPtr, input.length);
  return expected ? {ms} : result;
}
function check(actual, expected, label) {
  if (actual.succeeded !== expected.succeeded || actual.width !== expected.width || actual.height !== expected.height ||
      actual.error !== expected.error || !equal(actual.pixels, expected.pixels)) throw new Error(`Decode mismatch: ${label}`);
}
async function run() {
  const paths = ["means_l.webp", "means_u.webp", "quats.webp", "scales.webp", "sh0.webp", "shN_labels.webp", "shN_centroids.webp"];
  const root = "../levels/saburi-alley-yazd/1_0/";
  const [scalarBytes, simdBytes, ...files] = await Promise.all([
    get("../logs/simd-webp-probe/scalar.wasm"), get("../logs/simd-webp-probe/simd.wasm"), ...paths.map(p => get(root + p)),
  ]);
  const scalar = await WebAssembly.instantiate(scalarBytes);
  let simd, unsupported = null;
  if (!WebAssembly.validate(simdBytes)) unsupported = "SIMD module validation failed (unsupported feature or invalid module)";
  else simd = await WebAssembly.instantiate(simdBytes);
  const expected = [], fixtures = [], samples = [];
  for (let stream = 0; stream < files.length; stream++) {
    const source = files[stream], old = invoke(scalar, stream, source);
    if (!old.succeeded) throw new Error(`Valid WebP failed: ${paths[stream]}: ${old.error}`);
    if (simd) check(invoke(simd, stream, source), old, paths[stream]);
    expected.push(old);
    fixtures.push({file: paths[stream], width: old.width, height: old.height, inputBytes: source.length,
      outputBytes: old.pixels.length, inputSha256: await hash(source), outputSha256: await hash(old.pixels)});
    // Match missing optional SH and malformed/truncated inputs, including error text.
    for (const bad of [new Uint8Array(), source.slice(0, 4), source.slice(0, Math.floor(source.length / 2))]) {
      const a = invoke(scalar, stream, bad);
      if (simd) check(invoke(simd, stream, bad), a, `${paths[stream]} truncated to ${bad.length}`);
      fixtures.push({file: paths[stream], truncatedTo: bad.length, succeeded: a.succeeded, width: a.width, height: a.height,
        outputSha256: await hash(a.pixels), error: a.error});
    }
  }
  if (simd) {
    for (let round = 0; round < 2; round++) for (let s = 0; s < files.length; s++) {
      invoke(scalar, s, files[s], expected[s]); invoke(simd, s, files[s], expected[s]);
    }
    for (let round = 0; round < 12; round++) {
      const order = round % 2 ? [["simd", simd], ["scalar", scalar]] : [["scalar", scalar], ["simd", simd]];
      for (let s = 0; s < files.length; s++) for (const [variant, module] of order) {
        const result = invoke(module, s, files[s], expected[s]);
        samples.push({round, variant, stream: paths[s], ms: result.ms});
      }
    }
  }
  const result = {fixture: "Saburi 1_0 actual lossless WebP files", scalarModuleSha256: await hash(scalarBytes),
    simdModuleSha256: await hash(simdBytes), unsupported, fixtures, samples};
  output.textContent = JSON.stringify(result, null, 2);
  status.textContent = simd ? "Complete: all bytes match; decode timings recorded" : "SIMD unsupported; scalar validation complete";
  window.simdWebpDecodeResult = result;
}
run().catch(e => { status.textContent = `Failed: ${e}`; throw e; });
