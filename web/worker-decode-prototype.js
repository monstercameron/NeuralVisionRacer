// Diagnostic-only scheduler for a single scalar decoder worker. This is not wired into the game.
// The caller retains compressed bytes for fallback. One decoded chunk may stage while the worker decodes the next;
// all outstanding entries count toward maxPending, and staging/fallback callbacks remain FIFO.
export function createDecodeScheduler({ createWorker, onDecoded, onFallback, onInvalid, onCancel, maxPending = 2, timeoutMs = 30000, setTimer = setTimeout, clearTimer = clearTimeout }) {
  if (!Number.isInteger(maxPending) || maxPending < 1 || !(timeoutMs > 0)) throw new Error('invalid scheduler limits');
  if (typeof createWorker !== 'function' || typeof onDecoded !== 'function' || typeof onFallback !== 'function' || typeof onInvalid !== 'function' || typeof onCancel !== 'function') throw new Error('missing scheduler callback');
  let generation = 0, serial = 0, worker = null, disabled = false, disabledError = null;
  let decoding = null, staging = null, entries = [], pumping = false, pumpAgain = false;
  function stopWorker() {
    if (worker) { worker.onmessage = null; worker.onerror = null; worker.onmessageerror = null; worker.terminate(); worker = null; }
  }
  function clearDecode(item = decoding) {
    if (!item) return;
    if (item.timer != null) clearTimer(item.timer);
    item.timer = null;
    if (decoding === item) decoding = null;
  }
  function failTransport(error = new Error('worker transport failed')) {
    if (disabled) return;
    disabled = true;
    disabledError = error;
    stopWorker();
    const failed = decoding;
    clearDecode(failed);
    for (const item of entries) {
      if (item === failed || item.status === 'queued') {
        item.status = 'fallback';
        item.error = error;
      }
    }
    pump();
  }
  function settleStage(item) {
    if (staging !== item || entries[0] !== item) return;
    staging = null;
    item.outputs = null;
    item.input = null;
    entries.shift();
    pump();
  }
  function rejectStage(item, error) {
    if (staging !== item || entries[0] !== item) return;
    staging = null;
    item.outputs = null;
    item.status = 'invalid';
    item.error = error;
    pump();
  }
  function startStage(item) {
    item.status = 'staging';
    staging = item;
    let result;
    try {
      // Keep this FIFO: only the head entry is passed to the stager, even if a later decode is ready.
      result = onDecoded(item.input, item.outputs);
    } catch (error) {
      rejectStage(item, error);
      return;
    }
    Promise.resolve(result).then(() => settleStage(item), error => rejectStage(item, error));
  }
  function pumpDecode() {
    if (disabled || decoding) return;
    // Permit one decode while an earlier chunk stages. Once that decode is ready (or invalid/fallback),
    // wait for the older stage to finish before decoding another, keeping decoded memory bounded.
    if (entries.some(item => item.status === 'decoded' || item.status === 'invalid' || item.status === 'fallback')) return;
    const item = entries.find(entry => entry.status === 'queued');
    if (!item) return;
    try {
      if (!worker) {
        worker = createWorker();
        const owner = worker;
        const ownerGeneration = generation;
        const ownerFailed = () => { if (worker === owner && generation === ownerGeneration) failTransport(); };
        worker.onerror = ownerFailed;
        worker.onmessageerror = ownerFailed;
        worker.onmessage = ({ data }) => {
          if (worker !== owner || generation !== ownerGeneration || !decoding || data?.generation !== generation || data?.id !== decoding.id) return;
          if (data.error) { failTransport(new Error(`worker decode failed: ${data.error}`)); return; }
          const decoded = decoding;
          clearDecode(decoded);
          try {
            validateDecodedStreams(data.outputs, decoded.input.hasSh);
            decoded.outputs = data.outputs;
            decoded.status = 'decoded';
          } catch (error) {
            decoded.status = 'invalid';
            decoded.error = error;
          }
          pump();
        };
      }
      const id = ++serial;
      item.id = id;
      item.status = 'decoding';
      decoding = item;
      const ownerGeneration = generation;
      item.timer = setTimer(() => { if (generation === ownerGeneration && decoding === item) failTransport(new Error('worker timed out')); }, timeoutMs);
      worker.postMessage({ generation, id, files: item.input.files });
    } catch (_) {
      // A constructor or postMessage failure leaves the original compressed input intact.
      if (decoding === item) clearDecode(item);
      item.status = 'fallback';
      item.error = new Error('worker creation or postMessage failed');
      failTransport(item.error);
    }
  }
  function pump() {
    if (pumping) { pumpAgain = true; return; }
    pumping = true;
    let callbackError = null;
    try {
      do {
        pumpAgain = false;
        while (!staging && entries.length) {
          const head = entries[0];
          if (head.status === 'decoded') {
            startStage(head);
            break;
          }
          if (head.status !== 'invalid' && head.status !== 'fallback') break;
          entries.shift();
          const input = head.input, error = head.error;
          head.input = null;
          head.outputs = null;
          try {
            if (head.status === 'invalid') onInvalid(input, error);
            else onFallback(input, error);
          } catch (callbackFailure) {
            callbackError ||= callbackFailure;
          }
          pumpAgain = true;
        }
        try { pumpDecode(); } catch (error) { callbackError ||= error; }
      } while (pumpAgain);
    } finally {
      pumping = false;
    }
    if (callbackError) throw callbackError;
  }
  return {
    submit(input) {
      if (!input || !Array.isArray(input.files) || input.files.length !== 7 || typeof input.hasSh !== 'boolean') throw new Error('expected seven compressed streams and SH flag');
      if (entries.length >= maxPending) return false;
      entries.push({ input, status: disabled ? 'fallback' : 'queued', outputs: null, error: disabledError, id: null, timer: null });
      pump(); return true;
    },
    reset() {
      generation++;
      stopWorker();
      clearDecode();
      const cancelled = entries;
      entries = [];
      staging = null;
      disabled = false;
      disabledError = null;
      let callbackError = null;
      for (const item of cancelled) {
        const input = item.input;
        item.input = null;
        item.outputs = null;
        if (!input) continue;
        try { onCancel(input); } catch (error) { callbackError ||= error; }
      }
      if (callbackError) throw callbackError;
    },
    pending() { return entries.length; },
  };
}

// Treat worker output as untrusted before passing it to the viewer; SH placeholders may be empty.
export function validateDecodedStreams(outputs, hasSh) {
  if (!Array.isArray(outputs) || outputs.length !== 7) throw new Error('expected seven decoded streams');
  for (let i = 0; i < outputs.length; i++) {
    const { width, height, result } = outputs[i] || {};
    if (!Number.isSafeInteger(width) || !Number.isSafeInteger(height) || width < 0 || height < 0 || !(result instanceof Uint8Array)) throw new Error(`invalid texture ${i}`);
    if ((!width || !height) && !(i >= 5 && !hasSh && width === 0 && height === 0)) throw new Error(`empty texture ${i}`);
    const size = width * height * 4;
    if (!Number.isSafeInteger(size) || size !== result.byteLength) throw new Error(`invalid texture size ${i}`);
  }
}

// Diagnostic-only exact comparison of one main-thread decoded texture with its worker result.
export function compareDecodedTexture(mainWidth, mainHeight, mainPixels, workerOutput) {
  if (!Number.isSafeInteger(mainWidth) || !Number.isSafeInteger(mainHeight) || mainWidth < 0 || mainHeight < 0 || !(mainPixels instanceof Uint8Array)) throw new Error('invalid main-thread texture');
  if (!workerOutput || !Number.isSafeInteger(workerOutput.width) || !Number.isSafeInteger(workerOutput.height) || !(workerOutput.result instanceof Uint8Array)) throw new Error('invalid worker texture');
  const comparedBytes = Math.min(mainPixels.byteLength, workerOutput.result.byteLength);
  let differentBytes = 0, firstMismatchByte = null;
  for (let byte = 0; byte < comparedBytes; byte++) {
    if (mainPixels[byte] !== workerOutput.result[byte]) {
      differentBytes++;
      if (firstMismatchByte === null) firstMismatchByte = byte;
    }
  }
  if (firstMismatchByte === null && mainPixels.byteLength !== workerOutput.result.byteLength) firstMismatchByte = comparedBytes;
  const dimensionsMatch = mainWidth === workerOutput.width && mainHeight === workerOutput.height;
  const byteLengthsMatch = mainPixels.byteLength === workerOutput.result.byteLength;
  const rgbaEqual = byteLengthsMatch && differentBytes === 0;
  return {
    dimensions_match: dimensionsMatch,
    byte_lengths_match: byteLengthsMatch,
    rgba_equal: rgbaEqual,
    match: dimensionsMatch && rgbaEqual,
    main: { width: mainWidth, height: mainHeight, rgba_bytes: mainPixels.byteLength },
    worker: { width: workerOutput.width, height: workerOutput.height, rgba_bytes: workerOutput.result.byteLength },
    compared_bytes: comparedBytes,
    different_bytes: differentBytes,
    first_mismatch_byte: firstMismatchByte,
  };
}

// Stages at most one texture per submitted frame; abort discards the private Rust chunk before rejection.
export function createFrameStager(viewer, record = () => {}) {
  let stage = null;
  return {
    start(input, outputs) {
      if (stage) throw new Error('worker chunk already staging');
      return new Promise((resolve, reject) => { stage = { input, outputs, index: 0, resolve, reject }; });
    },
    tick() {
      if (!stage) return;
      const current = stage, start = performance.now();
      try {
        if (current.index === 0) viewer.worker_chunk_begin(current.input.file, current.input.meta);
        const index = current.index;
        const texture = current.outputs[index];
        viewer.worker_chunk_texture(current.index, texture.width, texture.height, texture.result);
        // Vec<u8> conversion copies into WASM. Release this JS buffer as soon as that copy succeeds.
        current.outputs[index] = null;
        current.index++;
        if (current.index === 7) { stage = null; current.resolve(); }
      } catch (error) {
        viewer.worker_chunk_abort(); stage = null; current.reject(error);
      } finally { record(performance.now() - start); }
    },
    cancel(input) {
      if (!stage || stage.input !== input) return;
      const current = stage; stage = null;
      viewer.worker_chunk_abort(); current.reject(new Error('worker staging cancelled'));
    },
  };
}
