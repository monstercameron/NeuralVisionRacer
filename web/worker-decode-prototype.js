// Diagnostic-only scheduler for a single scalar decoder worker. This is not wired into the game.
// The caller retains the compressed bytes; postMessage clones them so transport failures can retry unchanged.
// onDecoded must validate against SOG metadata and commit atomically. Rejected decoded output is sent to onInvalid,
// where the caller should release the stream's in-flight file; worker/protocol failures go to onFallback. A partial
// commit must be rolled back before rejecting. The current worker protocol does not distinguish init from decode errors.
export function createDecodeScheduler({ createWorker, onDecoded, onFallback, onInvalid, onCancel, maxPending = 2, timeoutMs = 30000, setTimer = setTimeout, clearTimer = clearTimeout }) {
  if (!Number.isInteger(maxPending) || maxPending < 1 || !(timeoutMs > 0)) throw new Error('invalid scheduler limits');
  if (typeof createWorker !== 'function' || typeof onDecoded !== 'function' || typeof onFallback !== 'function' || typeof onInvalid !== 'function' || typeof onCancel !== 'function') throw new Error('missing scheduler callback');
  let generation = 0, serial = 0, worker = null, disabled = false, active = null, queue = [];
  function stopWorker() {
    if (worker) { worker.onmessage = null; worker.onerror = null; worker.onmessageerror = null; worker.terminate(); worker = null; }
  }
  function clearActive() {
    if (active?.timer != null) clearTimer(active.timer);
    const item = active; active = null; return item;
  }
  function failTransport(error = new Error('worker transport failed')) {
    disabled = true;
    stopWorker();
    const item = clearActive();
    try {
      if (item) onFallback(item.input, error);
    } finally {
      // A caller callback should not strand already-queued inputs if it throws.
      pump();
    }
  }
  function rejectContent(item, error) {
    if (active !== item) return;
    clearActive();
    try {
      onInvalid(item.input, error);
    } finally {
      pump();
    }
  }
  function pump() {
    if (active || !queue.length) return;
    const input = queue.shift();
    if (disabled) {
      try { onFallback(input, new Error('worker disabled after transport failure')); }
      finally { pump(); }
      return;
    }
    try {
      if (!worker) {
        worker = createWorker();
        const owner = worker;
        const ownerGeneration = generation;
        const ownerFailed = () => { if (worker === owner && generation === ownerGeneration) failTransport(); };
        worker.onerror = ownerFailed;
        worker.onmessageerror = ownerFailed;
        worker.onmessage = ({ data }) => {
          if (worker !== owner || !active || data?.generation !== generation || data?.id !== active.id) return;
          const item = active;
          if (data.error) { failTransport(new Error(`worker decode failed: ${data.error}`)); return; }
          try {
            validateDecodedStreams(data.outputs, item.input.hasSh);
          } catch (error) {
            rejectContent(item, error);
            return;
          }
          if (item.timer != null) { clearTimer(item.timer); item.timer = null; }
          // Keep the chunk active until staged pixels have been committed or rolled back.
          try {
            Promise.resolve(onDecoded(item.input, data.outputs)).then(() => {
              if (active !== item) return;
              clearActive(); pump();
            }, error => { if (active === item) rejectContent(item, error); });
          } catch (error) { rejectContent(item, error); }
        };
      }
      const id = ++serial;
      active = { input, id, timer: null };
      const timerGeneration = generation;
      active.timer = setTimer(() => { if (generation === timerGeneration && active?.id === id) failTransport(new Error('worker timed out')); }, timeoutMs);
      worker.postMessage({ generation, id, files: input.files });
    } catch (_) {
      // A constructor or postMessage failure leaves the original compressed input intact.
      if (!active) active = { input, timer: null };
      failTransport(new Error('worker creation or postMessage failed'));
    }
  }
  return {
    submit(input) {
      if (!input || !Array.isArray(input.files) || input.files.length !== 7 || typeof input.hasSh !== 'boolean') throw new Error('expected seven compressed streams and SH flag');
      if (queue.length + Number(active !== null) >= maxPending) return false;
      queue.push(input); pump(); return true;
    },
    reset() {
      generation++;
      stopWorker();
      const item = clearActive();
      const cancelled = item ? [item.input, ...queue] : queue;
      queue = []; disabled = false;
      for (const input of cancelled) onCancel(input);
    },
    pending() { return queue.length + Number(active !== null); },
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
