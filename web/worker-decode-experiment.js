// Opt-in only. Install the message handler before WASM initialization so early messages are queued.
import init, { worker_decode_texture } from './pkg/splat_web.js';
const ready = init();
onmessage = async ({data: {generation, id, files}}) => {
  try {
    await ready;
    const outputs = files.map((bytes, index) => {
      const texture = worker_decode_texture(index, bytes);
      try { return {width: texture.width(), height: texture.height(), result: texture.take_pixels()}; }
      finally { texture.free(); }
    });
    postMessage({generation,id,outputs}, outputs.map(output => output.result.buffer));
  } catch (error) { postMessage({generation,id,error:String(error)}); }
};
