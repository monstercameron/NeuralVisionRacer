// First gate only: no benchmark, GPU, canvas, game state, or persistent storage.
// ImageDecoder's "none" applies at image decode. WebCodecs explicitly converts
// to a target RGB color space when copyTo({format: "RGBA"}) is requested, so
// also check a native-format copy, with only channel-order/opaque-alpha fixes.
// https://www.w3.org/TR/webcodecs/#dom-videoframe-copyto
// https://www.w3.org/TR/webcodecs/#dictdef-videoframecopytooptions
const status = document.querySelector("#status"), output = document.querySelector("#result");
const params = new URLSearchParams(location.search);
const level = params.get("level") || "saburi-alley-yazd", chunk = params.get("chunk") || "1_0";
if (!/^[a-zA-Z0-9_-]+$/.test(level) || !/^[a-zA-Z0-9_-]+$/.test(chunk)) throw new Error("Invalid level or chunk");
const filenames = ["means_l.webp", "means_u.webp", "quats.webp", "scales.webp", "sh0.webp", "shN_labels.webp", "shN_centroids.webp"];
const get = async path => {
  const response = await fetch(path, {cache: "no-store"});
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
};
const hash = async bytes => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), x => x.toString(16).padStart(2, "0")).join("");
function wasmDecode(api, stream, bytes) {
  const input = api.reserve(bytes.length);
  new Uint8Array(api.memory.buffer, input, bytes.length).set(bytes);
  const out = api.decode(stream, input, bytes.length);
  try {
    const ok = !!api.succeeded(out), width = api.width(out), height = api.height(out);
    const pixelsLength = api.pixels_len(out), pixelsPtr = api.pixels(out);
    const errorLength = api.error_len(out), errorPtr = api.error(out);
    const pixels = pixelsLength ? new Uint8Array(api.memory.buffer, pixelsPtr, pixelsLength).slice() : new Uint8Array();
    const error = errorLength ? new TextDecoder().decode(new Uint8Array(api.memory.buffer, errorPtr, errorLength)) : "";
    return {ok, width, height, pixels, error};
  } finally {
    api.release_outcome(out);
    api.release(input, bytes.length);
  }
}
function normalizeLabels(bytes) {
  const out = bytes.slice();
  for (let i = 0; i + 3 < out.length; i += 4) out[i + 2] = 0;
  return out;
}
function alphaCounts(bytes, activePixels) {
  const counts = {zero: {pixels: 0, activePixels: 0, nonzeroRgb: 0},
    partial: {pixels: 0, activePixels: 0, nonzeroRgb: 0}, opaque: {pixels: 0, activePixels: 0, nonzeroRgb: 0}};
  for (let i = 0; i + 3 < bytes.length; i += 4) {
    const category = bytes[i + 3] === 0 ? "zero" : bytes[i + 3] === 255 ? "opaque" : "partial";
    counts[category].pixels++;
    if (i / 4 < activePixels) counts[category].activePixels++;
    if (bytes[i] || bytes[i + 1] || bytes[i + 2]) counts[category].nonzeroRgb++;
  }
  return counts;
}
function compare(reference, actual, width, height, activePixels = width * height) {
  const channelsChanged = [0, 0, 0, 0];
  const alpha = {zero: {pixels: 0, activePixels: 0, changedPixels: 0, activeChangedPixels: 0},
    partial: {pixels: 0, activePixels: 0, changedPixels: 0, activeChangedPixels: 0},
    opaque: {pixels: 0, activePixels: 0, changedPixels: 0, activeChangedPixels: 0}};
  const firstMismatches = [];
  let changedPixels = 0, maxDelta = 0;
  if (reference.length !== actual.length || reference.length !== width * height * 4) {
    return {identical: false, expectedBytes: reference.length, actualBytes: actual.length, reason: "byte length mismatch"};
  }
  for (let i = 0; i < reference.length; i += 4) {
    const category = reference[i + 3] === 0 ? "zero" : reference[i + 3] === 255 ? "opaque" : "partial";
    alpha[category].pixels++;
    if (i / 4 < activePixels) alpha[category].activePixels++;
    let pixelChanged = false;
    for (let c = 0; c < 4; c++) {
      const delta = Math.abs(reference[i + c] - actual[i + c]);
      if (delta) { channelsChanged[c]++; pixelChanged = true; maxDelta = Math.max(maxDelta, delta); }
    }
    if (pixelChanged) {
      changedPixels++;
      alpha[category].changedPixels++;
      if (i / 4 < activePixels) alpha[category].activeChangedPixels++;
      if (firstMismatches.length < 12) firstMismatches.push({x: (i / 4) % width, y: Math.floor(i / (4 * width)),
        scalar: Array.from(reference.subarray(i, i + 4)), native: Array.from(actual.subarray(i, i + 4))});
    }
  }
  return {identical: changedPixels === 0, bytes: reference.length, changedPixels, channelsChanged, maxDelta, alpha, firstMismatches};
}
async function copyFrame(frame, kind) {
  const rect = frame.visibleRect;
  if (!rect || !Number.isInteger(rect.width) || !Number.isInteger(rect.height)) throw new Error("Invalid visible rectangle");
  const nativeFormat = frame.format;
  if (kind === "native" && !["RGBA", "RGBX", "BGRA", "BGRX"].includes(nativeFormat)) {
    return {available: false, reason: `Native pixel layout ${String(nativeFormat)} is not packed RGBA/BGRA`};
  }
  // Specifying RGBA asks WebCodecs to convert to RGB with sRGB as its default
  // target. Omitting format copies the source layout; neither path uses canvas.
  const options = kind === "requested_rgba" ? {format: "RGBA"} : {};
  const raw = new Uint8Array(frame.allocationSize(options));
  const planes = await frame.copyTo(raw, options);
  if (planes.length !== 1) throw new Error(`Expected one packed plane, got ${planes.length}`);
  const {offset, stride} = planes[0], width = rect.width, height = rect.height;
  if (stride < width * 4 || offset + Math.max(0, height - 1) * stride + width * 4 > raw.length) throw new Error("Invalid packed plane layout");
  const bytes = new Uint8Array(width * height * 4), format = kind === "requested_rgba" ? "RGBA" : nativeFormat;
  for (let y = 0; y < height; y++) {
    const start = offset + y * stride;
    bytes.set(raw.subarray(start, start + width * 4), y * width * 4);
  }
  if (format === "BGRA" || format === "BGRX") {
    for (let i = 0; i < bytes.length; i += 4) { const r = bytes[i]; bytes[i] = bytes[i + 2]; bytes[i + 2] = r; }
  }
  if (format === "RGBX" || format === "BGRX") for (let i = 3; i < bytes.length; i += 4) bytes[i] = 255;
  return {available: true, width, height, offset, stride, bytes};
}
async function nativeDecode(source) {
  const decoder = new ImageDecoder({type: "image/webp", data: source, colorSpaceConversion: "none", preferAnimation: false});
  let frame;
  try {
    const decoded = await decoder.decode({frameIndex: 0, completeFramesOnly: true});
    frame = decoded.image;
    if (!decoded.complete) throw new Error("ImageDecoder returned an incomplete frame");
    const rect = frame.visibleRect;
    const details = {format: frame.format, codedWidth: frame.codedWidth, codedHeight: frame.codedHeight,
      displayWidth: frame.displayWidth, displayHeight: frame.displayHeight,
      visibleRect: rect ? {x: rect.x, y: rect.y, width: rect.width, height: rect.height} : null,
      rotation: frame.rotation ?? 0, flip: frame.flip ?? false,
      colorSpace: frame.colorSpace && typeof frame.colorSpace.toJSON === "function" ? frame.colorSpace.toJSON() : null};
    const copies = {};
    for (const kind of ["requested_rgba", "native"]) {
      try { copies[kind] = await copyFrame(frame, kind); }
      catch (error) { copies[kind] = {available: false, reason: String(error)}; }
    }
    return {details, copies};
  } finally {
    // A failed decode can already have closed the decoder.
    if (frame) { try { frame.close(); } catch (_) {} }
    try { decoder.close(); } catch (_) {}
  }
}
function finish(result, text) {
  window.nativeWebpFidelityResult = result;
  output.textContent = JSON.stringify(result, null, 2);
  status.textContent = text;
}
async function run() {
  const capability = {secureContext: isSecureContext, imageDecoder: typeof ImageDecoder !== "undefined", webpSupported: false};
  if (!capability.imageDecoder || typeof ImageDecoder.isTypeSupported !== "function") {
    finish({status: "unsupported", capability, reason: "ImageDecoder or its type-support query is unavailable", timings: "not run"}, "Unsupported; use the scalar decoder");
    return;
  }
  try { capability.webpSupported = await ImageDecoder.isTypeSupported("image/webp"); }
  catch (error) {
    finish({status: "unsupported", capability, reason: `WebP support query failed: ${error}`, timings: "not run"}, "Unsupported; use the scalar decoder");
    return;
  }
  if (!capability.webpSupported) {
    finish({status: "unsupported", capability, reason: "ImageDecoder does not support WebP", timings: "not run"}, "Unsupported; use the scalar decoder");
    return;
  }
  const [wasm, metaBytes] = await Promise.all([get("../logs/simd-webp-probe/scalar.wasm"), get(`../levels/${level}/${chunk}/meta.json`)]);
  const meta = JSON.parse(new TextDecoder().decode(metaBytes));
  const api = (await WebAssembly.instantiate(wasm)).instance.exports;
  const cases = [];
  for (let i = 0; i < filenames.length; i++) {
    const filename = filenames[i];
    status.textContent = `Checking ${filename} (${i + 1}/${filenames.length})…`;
    const source = await get(`../levels/${level}/${chunk}/${filename}`);
    // SOG intentionally zeros labels' blue byte. Decode this same file through
    // means_l once to compare the unmodified WebP data first, then compare the
    // real sh_labels path after normalization as a second, independent check.
    const raw = wasmDecode(api, i === 5 ? 0 : i, source);
    const normalized = i === 5 ? wasmDecode(api, 5, source) : raw;
    if (!raw.ok || !normalized.ok) throw new Error(`Scalar decode failed for ${filename}: ${raw.error || normalized.error}`);
    if (i === 5 && (raw.width !== normalized.width || raw.height !== normalized.height ||
      !compare(normalizeLabels(raw.pixels), normalized.pixels, raw.width, raw.height).identical)) {
      throw new Error("Scalar label normalization did not match the raw decode");
    }
    const declaredActive = i === 6 ? Number(meta.shN?.count ?? 0) * 15 : Number(meta.count ?? 0);
    const activePixels = Math.max(0, Math.min(raw.width * raw.height, declaredActive));
    const entry = {file: filename, inputBytes: source.length, inputSha256: await hash(source), scalar: {
      width: raw.width, height: raw.height, activePixels, alpha: alphaCounts(raw.pixels, activePixels),
      rawSha256: await hash(raw.pixels), normalizedSha256: await hash(normalized.pixels)},
      native: null, copies: {}};
    try {
      const native = await nativeDecode(source);
      entry.native = native.details;
      for (const kind of ["requested_rgba", "native"]) {
        const copy = native.copies[kind];
        if (!copy.available) { entry.copies[kind] = copy; continue; }
        const dimensionsMatch = copy.width === raw.width && copy.height === raw.height &&
          native.details.rotation === 0 && native.details.flip === false;
        const rawComparison = compare(raw.pixels, copy.bytes, raw.width, raw.height, activePixels);
        const normalizedBytes = i === 5 ? normalizeLabels(copy.bytes) : copy.bytes;
        const normalizedComparison = compare(normalized.pixels, normalizedBytes, normalized.width, normalized.height, activePixels);
        entry.copies[kind] = {available: true, width: copy.width, height: copy.height, offset: copy.offset, stride: copy.stride,
          dimensionsMatch, rawSha256: await hash(copy.bytes), normalizedSha256: await hash(normalizedBytes), rawComparison, normalizedComparison};
      }
    } catch (error) { entry.nativeError = String(error); }
    cases.push(entry);
  }
  const exact = kind => cases.every(c => {
    const v = c.copies[kind];
    return !c.nativeError && v && v.available && v.dimensionsMatch && v.rawComparison.identical && v.normalizedComparison.identical;
  });
  const requestedRgbaExact = exact("requested_rgba"), nativeLayoutExact = exact("native");
  finish({status: requestedRgbaExact ? "rgba_exact" : "rgba_mismatch_or_unavailable", level, chunk, capability,
    scalarModuleSha256: await hash(wasm), metaSha256: await hash(metaBytes), requestedRgbaExact, nativeLayoutExact, cases,
    timings: "not run: fidelity gate only; shared CPU not isolated"},
  requestedRgbaExact ? "RGBA matches every byte in all streams" : "RGBA differs or is unavailable; see per-stream results");
}
run().catch(error => {
  finish({status: "error", error: String(error), level, chunk, timings: "not run"}, `Diagnostic failed: ${error}`);
  throw error;
});
