import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compareDecodedTexture, createDecodeScheduler, validateDecodedStreams } from '../web/worker-decode-prototype.js';

function fixture(options = {}) {
  const workers = [], decoded = [], fallback = [], invalid = [], cancelled = [], timers = new Set();
  const s = createDecodeScheduler({ createWorker: () => { const w = { messages: [], terminated: false, postMessage(m) { this.messages.push(m); }, terminate() { this.terminated = true; } }; workers.push(w); return w; },
    onDecoded: (item, outputs) => { validateDecodedStreams(outputs, true); decoded.push(item.id); },
    onFallback: item => fallback.push(item.id), onInvalid: (item, error) => invalid.push({id:item.id, error:String(error)}), onCancel: item => cancelled.push(item.id),
    setTimer: fn => { timers.add(fn); return fn; }, clearTimer: fn => timers.delete(fn), ...options });
  const item = id => ({ id, hasSh: true, files: Array.from({length: 7}, () => new Uint8Array([id])) });
  const output = () => Array.from({length: 7}, () => ({width: 1, height: 1, result: new Uint8Array(4)}));
  const reply = (w, outputs = output()) => { const { id, generation } = w.messages.at(-1); w.onmessage({data:{id,generation,outputs}}); };
  return {s, workers, decoded, fallback, invalid, cancelled, timers, item, output, reply};
}
test('decodes one chunk ahead, preserves order and bounds all pending chunks', async () => {
  const f=fixture(), a=f.item(1), b=f.item(2); assert.equal(f.s.submit(a),true); assert.equal(f.s.submit(b),true); assert.equal(f.s.submit(f.item(3)),false);
  const w=f.workers[0]; assert.equal(w.messages.length,1); f.reply(w); await Promise.resolve(); assert.deepEqual(f.decoded,[1]); assert.equal(w.messages.length,2); f.reply(w); await Promise.resolve(); assert.deepEqual(f.decoded,[1,2]); assert.equal(f.s.pending(),0); assert.equal(f.timers.size,0);
});

test('decode N+1 overlaps staging N, but staging and commit callbacks stay FIFO', async () => {
  const workers=[], started=[], committed=[]; let finishFirst;
  const scheduler=createDecodeScheduler({
    createWorker:()=>{const w={messages:[],postMessage(m){this.messages.push(m);},terminate(){}};workers.push(w);return w;},
    onDecoded:item=>{
      started.push(item.id);
      if(item.id===1)return new Promise(resolve=>{finishFirst=()=>{committed.push(1);resolve();};});
      committed.push(item.id);
    },
    onFallback:item=>assert.fail(`unexpected fallback for ${item.id}`),
    onInvalid:(item,error)=>assert.fail(`unexpected invalid chunk ${item.id}: ${error}`),
    onCancel:item=>assert.fail(`unexpected cancellation for ${item.id}`),
  });
  const item=id=>({id,hasSh:true,files:Array.from({length:7},()=>new Uint8Array())});
  const outputs=()=>Array.from({length:7},()=>({width:1,height:1,result:new Uint8Array(4)}));
  const reply=(worker,message)=>worker.onmessage({data:{generation:message.generation,id:message.id,outputs:outputs()}});
  scheduler.submit(item(1));scheduler.submit(item(2));
  const worker=workers[0], firstMessage=worker.messages[0];
  reply(worker,firstMessage);
  assert.deepEqual(started,[1]);
  assert.equal(worker.messages.length,2,"the next decode starts while chunk 1 is staging");
  const secondMessage=worker.messages[1];
  reply(worker,secondMessage);
  assert.deepEqual(started,[1],"chunk 2 waits for chunk 1's staging promise");
  assert.deepEqual(committed,[]);
  finishFirst();
  await new Promise(resolve=>setTimeout(resolve,0));
  assert.deepEqual(started,[1,2]);
  assert.deepEqual(committed,[1,2]);
  assert.equal(scheduler.pending(),0);
});
test('worker error falls back in order with compressed bytes retained', () => {
  const f=fixture(),a=f.item(1); f.s.submit(a); f.s.submit(f.item(2)); f.workers[0].onerror(); assert.deepEqual(f.fallback,[1,2]); assert.equal(a.files[0][0],1); assert.equal(f.workers[0].terminated,true); assert.equal(f.s.pending(),0); f.s.submit(f.item(3)); assert.deepEqual(f.fallback,[1,2,3]);
});
test('invalid decoded output is released while worker timeouts use fallback', () => {
  const f=fixture(); f.s.submit(f.item(1)); f.reply(f.workers[0],[]);
  assert.deepEqual(f.invalid.map(x=>x.id),[1]); assert.deepEqual(f.fallback,[]); assert.equal(f.s.pending(),0);
  f.s.submit(f.item(2)); [...f.timers][0](); assert.deepEqual(f.fallback,[2]);
});
test('content validation failure does not disable worker or strand queued chunks', async () => {
  const f=fixture(), first=f.item(1), second=f.item(2); f.s.submit(first); f.s.submit(second);
  const w=f.workers[0]; f.reply(w,[]);
  assert.deepEqual(f.invalid.map(x=>x.id),[1]); assert.deepEqual(f.fallback,[]);
  assert.equal(w.messages.length,2); f.reply(w); await Promise.resolve();
  assert.deepEqual(f.decoded,[2]); assert.equal(f.s.pending(),0);
});

test('invalid decode-ahead waits behind the active stage, then releases and continues', async () => {
  const workers=[], order=[];let finishFirst;
  const scheduler=createDecodeScheduler({
    createWorker:()=>{const w={messages:[],postMessage(m){this.messages.push(m);},terminate(){}};workers.push(w);return w;},
    onDecoded:item=>{order.push(`stage:${item.id}`);if(item.id===1)return new Promise(resolve=>{finishFirst=()=>{order.push('commit:1');resolve();};});order.push(`commit:${item.id}`);},
    onFallback:item=>order.push(`fallback:${item.id}`),
    onInvalid:item=>order.push(`invalid:${item.id}`),
    onCancel:item=>order.push(`cancel:${item.id}`),
    maxPending:3,
  });
  const item=id=>({id,hasSh:true,files:Array.from({length:7},()=>new Uint8Array())});
  const outputs=()=>Array.from({length:7},()=>({width:1,height:1,result:new Uint8Array(4)}));
  const workerReply=(worker,message,result)=>worker.onmessage({data:{generation:message.generation,id:message.id,outputs:result}});
  scheduler.submit(item(1));scheduler.submit(item(2));scheduler.submit(item(3));
  const worker=workers[0];
  workerReply(worker,worker.messages[0],outputs());
  assert.equal(worker.messages.length,2);
  workerReply(worker,worker.messages[1],[]);
  assert.deepEqual(order,['stage:1']);
  assert.equal(worker.messages.length,2,"the invalid ready item prevents decoding farther ahead");
  finishFirst();
  await new Promise(resolve=>setTimeout(resolve,0));
  assert.equal(worker.messages.length,3,"the next decode starts after invalid chunk 2 is released");
  workerReply(worker,worker.messages[2],outputs());
  await new Promise(resolve=>setTimeout(resolve,0));
  assert.deepEqual(order,['stage:1','commit:1','invalid:2','stage:3','commit:3']);
  assert.equal(scheduler.pending(),0);
});

test('worker crash during staging/decode-ahead falls back later chunks in FIFO order', async () => {
  const workers=[], order=[];let finishFirst;
  const scheduler=createDecodeScheduler({
    createWorker:()=>{const w={messages:[],terminated:false,postMessage(m){this.messages.push(m);},terminate(){this.terminated=true;}};workers.push(w);return w;},
    onDecoded:item=>{order.push(`stage:${item.id}`);return new Promise(resolve=>{finishFirst=()=>{order.push(`commit:${item.id}`);resolve();};});},
    onFallback:item=>order.push(`fallback:${item.id}`),
    onInvalid:(item,error)=>assert.fail(`unexpected invalid chunk ${item.id}: ${error}`),
    onCancel:item=>assert.fail(`unexpected cancellation for ${item.id}`),
    maxPending:3,
  });
  const item=id=>({id,hasSh:true,files:Array.from({length:7},()=>new Uint8Array([id]))});
  const outputs=()=>Array.from({length:7},()=>({width:1,height:1,result:new Uint8Array(4)}));
  scheduler.submit(item(1));scheduler.submit(item(2));scheduler.submit(item(3));
  const worker=workers[0], firstMessage=worker.messages[0];
  worker.onmessage({data:{...firstMessage,outputs:outputs()}});
  assert.equal(worker.messages.length,2);
  worker.onerror(new Error('worker crashed'));
  assert.deepEqual(order,['stage:1'],"fallback must wait for the older staged chunk");
  assert.equal(scheduler.pending(),3);
  assert.equal(worker.terminated,true);
  finishFirst();
  await new Promise(resolve=>setTimeout(resolve,0));
  assert.deepEqual(order,['stage:1','commit:1','fallback:2','fallback:3']);
  assert.equal(scheduler.pending(),0);
});

test('decode timeout during an older stage preserves FIFO fallback and original compressed bytes', async () => {
  const workers=[], timers=new Set(), order=[];let finishFirst;
  const scheduler=createDecodeScheduler({
    createWorker:()=>{const w={messages:[],postMessage(m){this.messages.push(m);},terminate(){}};workers.push(w);return w;},
    onDecoded:item=>{order.push(`stage:${item.id}`);return new Promise(resolve=>{finishFirst=()=>{order.push(`commit:${item.id}`);resolve();};});},
    onFallback:item=>order.push(`fallback:${item.id}:${item.files[0][0]}`),
    onInvalid:(item,error)=>assert.fail(`unexpected invalid chunk ${item.id}: ${error}`),
    onCancel:item=>assert.fail(`unexpected cancellation for ${item.id}`),
    maxPending:2,setTimer:fn=>{timers.add(fn);return fn;},clearTimer:fn=>timers.delete(fn),
  });
  const item=id=>({id,hasSh:true,files:Array.from({length:7},()=>new Uint8Array([id]))});
  const outputs=()=>Array.from({length:7},()=>({width:1,height:1,result:new Uint8Array(4)}));
  const first=item(1),second=item(2);
  scheduler.submit(first);scheduler.submit(second);
  const worker=workers[0],firstMessage=worker.messages[0];
  worker.onmessage({data:{...firstMessage,outputs:outputs()}});
  assert.equal(worker.messages.length,2);
  [...timers][0]();
  assert.deepEqual(order,['stage:1']);
  finishFirst();
  await new Promise(resolve=>setTimeout(resolve,0));
  assert.deepEqual(order,['stage:1','commit:1','fallback:2:2']);
  assert.equal(second.files[0][0],2);
  assert.equal(scheduler.pending(),0);
  assert.equal(timers.size,0);
});
test('staging validation rejection releases only that chunk and continues the queue', async () => {
  const f=fixture({onDecoded:(item, outputs)=>{
    validateDecodedStreams(outputs,true);
    if (item.id===1) return Promise.reject(new Error('metadata or texture validation failed'));
  }});
  f.s.submit(f.item(1)); f.s.submit(f.item(2));
  const w=f.workers[0]; f.reply(w); await Promise.resolve();
  assert.deepEqual(f.invalid.map(x=>x.id),[1]); assert.deepEqual(f.fallback,[]); assert.equal(w.messages.length,2);
  f.reply(w); await Promise.resolve(); assert.equal(f.s.pending(),0);
});
test('partial Rust-style staging rejection aborts the chunk and does not replay it', async () => {
  const {createFrameStager}=await import('../web/worker-decode-prototype.js');
  const workers=[], invalid=[], fallback=[], released=[], calls=[];
  let stagingFile=null;
  const viewer={
    worker_chunk_begin(file){stagingFile=file;calls.push(['begin',file]);},
    worker_chunk_texture(index){calls.push(['texture',stagingFile,index]);if(stagingFile===1&&index===2)throw new Error('invalid staged texture');},
    worker_chunk_abort(){calls.push(['abort']);stagingFile=null;},
    level_chunk_failed(file){released.push(file);},
  };
  const stager=createFrameStager(viewer);
  const scheduler=createDecodeScheduler({
    createWorker:()=>{const w={messages:[],postMessage(message){this.messages.push(message);},terminate(){}};workers.push(w);return w;},
    onDecoded:(input,outputs)=>stager.start(input,outputs),
    onFallback:input=>fallback.push(input.file),
    onInvalid:(input,error)=>{stager.cancel(input);viewer.worker_chunk_abort();viewer.level_chunk_failed(input.file);invalid.push([input.file,String(error)]);},
    onCancel:input=>{stager.cancel(input);viewer.worker_chunk_abort();viewer.level_chunk_failed(input.file);},
  });
  const item=id=>({id,file:id,meta:new Uint8Array(),hasSh:true,files:Array.from({length:7},()=>new Uint8Array())});
  const outputs=()=>Array.from({length:7},()=>({width:1,height:1,result:new Uint8Array(4)}));
  const reply=(worker)=>{const {id,generation}=worker.messages.at(-1);worker.onmessage({data:{id,generation,outputs:outputs()}});};
  scheduler.submit(item(1));scheduler.submit(item(2));
  const worker=workers[0];reply(worker);for(let i=0;i<3;i++)stager.tick();await Promise.resolve();
  assert.deepEqual(released,[1]);assert.deepEqual(invalid.map(([file])=>file),[1]);assert.deepEqual(fallback,[]);
  assert.equal(worker.messages.length,2);reply(worker);for(let i=0;i<7;i++)stager.tick();await Promise.resolve();
  assert.equal(scheduler.pending(),0);assert.deepEqual(released,[1]);assert.deepEqual(fallback,[]);
  assert.deepEqual(calls.slice(0,5),[['begin',1],['texture',1,0],['texture',1,1],['texture',1,2],['abort']]);
  assert.deepEqual(calls.slice(-8),[['begin',2],...Array.from({length:7},(_,i)=>['texture',2,i])]);
});
test('reset cancels active and queued work, and stale replies cannot cross generations', async () => {
  const f=fixture(); f.s.submit(f.item(1)); f.s.submit(f.item(2)); const old=f.workers[0], oldHandler=old.onmessage, oldError=old.onerror, oldTimer=[...f.timers][0], oldMessage=old.messages[0]; f.s.reset(); assert.deepEqual(f.cancelled,[1,2]); f.s.submit(f.item(3)); oldHandler({data:{...oldMessage,outputs:f.output()}}); oldError(); oldTimer(); assert.deepEqual(f.decoded,[]); assert.deepEqual(f.fallback,[]); f.reply(f.workers[1]); await Promise.resolve(); assert.deepEqual(f.decoded,[3]);
});

test('reset during stage plus decode-ahead cancels each input and ignores both late completions', async () => {
  const workers=[], cancelled=[], staged=[];let finishOld;
  const scheduler=createDecodeScheduler({
    createWorker:()=>{const w={messages:[],postMessage(m){this.messages.push(m);},terminate(){}};workers.push(w);return w;},
    onDecoded:item=>{staged.push(item.id);if(item.id===1)return new Promise(resolve=>{finishOld=resolve;});},
    onFallback:item=>assert.fail(`unexpected fallback for ${item.id}`),
    onInvalid:(item,error)=>assert.fail(`unexpected invalid chunk ${item.id}: ${error}`),
    onCancel:item=>cancelled.push(item.id),
    maxPending:2,
  });
  const item=id=>({id,hasSh:true,files:Array.from({length:7},()=>new Uint8Array())});
  const outputs=()=>Array.from({length:7},()=>({width:1,height:1,result:new Uint8Array(4)}));
  scheduler.submit(item(1));scheduler.submit(item(2));
  const old=workers[0],oldHandler=old.onmessage,firstMessage=old.messages[0];
  old.onmessage({data:{...firstMessage,outputs:outputs()}});
  assert.equal(old.messages.length,2);
  const secondMessage=old.messages[1];
  scheduler.reset();
  assert.deepEqual(cancelled,[1,2]);
  assert.equal(scheduler.pending(),0);
  oldHandler({data:{...secondMessage,outputs:outputs()}});
  finishOld();
  await new Promise(resolve=>setTimeout(resolve,0));
  assert.deepEqual(staged,[1]);
  assert.equal(scheduler.pending(),0);
});
test('validates dimensions, byte sizes and optional SH placeholders', () => {
  const f=fixture(), out=f.output(); assert.doesNotThrow(()=>validateDecodedStreams(out,true)); out[5]={width:0,height:0,result:new Uint8Array()}; out[6]={width:0,height:0,result:new Uint8Array()}; assert.doesNotThrow(()=>validateDecodedStreams(out,false)); assert.throws(()=>validateDecodedStreams(out,true)); out[0]={width:2,height:2,result:new Uint8Array(4)}; assert.throws(()=>validateDecodedStreams(out,false));
});
test('compares texture dimensions and every RGBA byte', () => {
  const pixels=new Uint8Array([0,1,2,3,4,5,6,7]);
  const equal=compareDecodedTexture(2,1,pixels,{width:2,height:1,result:new Uint8Array(pixels)});
  assert.equal(equal.match,true);assert.equal(equal.compared_bytes,8);assert.equal(equal.different_bytes,0);
  const changed=compareDecodedTexture(2,1,pixels,{width:2,height:1,result:new Uint8Array([0,9,2,3,4,5,6,8])});
  assert.equal(changed.match,false);assert.equal(changed.rgba_equal,false);assert.equal(changed.different_bytes,2);assert.equal(changed.first_mismatch_byte,1);
  const dimensions=compareDecodedTexture(2,1,pixels,{width:1,height:2,result:new Uint8Array(pixels)});
  assert.equal(dimensions.dimensions_match,false);assert.equal(dimensions.rgba_equal,true);assert.equal(dimensions.match,false);
  const length=compareDecodedTexture(2,1,pixels,{width:2,height:1,result:new Uint8Array([...pixels,9])});
  assert.equal(length.byte_lengths_match,false);assert.equal(length.first_mismatch_byte,8);assert.equal(length.match,false);
});
test('constructor and postMessage failures retry the original chunk', () => {
  for (const createWorker of [() => { throw Error('unavailable'); }, () => ({ postMessage() { throw Error('clone failed'); }, terminate() {} })]) {
    const f=fixture({createWorker}); const a=f.item(4); assert.equal(f.s.submit(a),true); assert.deepEqual(f.fallback,[4]); assert.equal(a.files[0][0],4); assert.equal(f.s.pending(),0);
  }
});
test('ignores out-of-order and duplicate replies without advancing the wrong input', async () => {
  const f=fixture(); f.s.submit(f.item(1)); f.s.submit(f.item(2));
  const w=f.workers[0], first={...w.messages[0]};
  w.onmessage({data:{generation:first.generation,id:first.id+1,outputs:f.output()}});
  assert.equal(f.s.pending(),2);assert.deepEqual(f.decoded,[]);
  w.onmessage({data:{...first,outputs:f.output()}});
  await Promise.resolve();
  assert.deepEqual(f.decoded,[1]);assert.equal(w.messages.length,2);
  const second={...w.messages[1]};
  const pendingBeforeDuplicate=f.s.pending();
  w.onmessage({data:{...first,outputs:f.output()}});
  assert.equal(f.s.pending(),pendingBeforeDuplicate);assert.deepEqual(f.decoded,[1]);
  w.onmessage({data:{...second,outputs:f.output()}});
  await Promise.resolve();
  assert.deepEqual(f.decoded,[1,2]);assert.equal(f.s.pending(),0);
});

test('abort during staged ingestion prevents later frames from committing stale pixels', async () => {
  const {createFrameStager}=await import('../web/worker-decode-prototype.js');
  const calls=[];
  const viewer={worker_chunk_begin(file){calls.push(['begin',file]);},worker_chunk_texture(i,width,height,rgba){assert.equal(rgba.length,4);calls.push(['texture',i]);},worker_chunk_abort(){calls.push(['abort']);}};
  const stager=createFrameStager(viewer), first={file:1}, second={file:2};
  const makeTextures=()=>Array.from({length:7},()=>({width:1,height:1,result:new Uint8Array(4)}));
  const firstTextures=makeTextures();
  const old=stager.start(first,firstTextures); old.catch(()=>{}); stager.tick(); assert.equal(firstTextures[0],null); stager.cancel(first); stager.tick();
  assert.deepEqual(calls,[['begin',1],['texture',0],['abort']]);
  const secondTextures=makeTextures(), current=stager.start(second,secondTextures); stager.cancel(first);
  for(let i=0;i<7;i++){stager.tick();assert.equal(secondTextures[i],null);}
  await current;
  assert.deepEqual(calls.slice(-8),[['begin',2],...Array.from({length:7},(_,i)=>['texture',i])]);
});
test('late async commit completion cannot affect a new generation', async () => {
  for (const rejectOld of [false, true]) {
    let finishOld, finishNew;
    const cancelled=[],fallback=[],workers=[];
    const scheduler=createDecodeScheduler({createWorker:()=>{const w={messages:[],postMessage(m){this.messages.push(m);},terminate(){}};workers.push(w);return w;},
      onDecoded:item=>new Promise((resolve,reject)=>{if(item.id===1)finishOld=rejectOld ? reject : resolve;else finishNew=resolve;}),
      onFallback:item=>fallback.push(item.id),onInvalid:item=>assert.fail(`unexpected content rejection for ${item.id}`),onCancel:item=>cancelled.push(item.id)});
    const item=id=>({id,hasSh:true,files:Array.from({length:7},()=>new Uint8Array())});
    const outputs=Array.from({length:7},()=>({width:1,height:1,result:new Uint8Array(4)}));
    scheduler.submit(item(1)); let w=workers[0], msg=w.messages[0]; w.onmessage({data:{...msg,outputs}}); scheduler.reset();
    scheduler.submit(item(2)); w=workers[1]; msg=w.messages[0]; w.onmessage({data:{...msg,outputs}});
    finishOld(Error('late')); await Promise.resolve(); assert.equal(scheduler.pending(),1); assert.deepEqual(fallback,[]); assert.deepEqual(cancelled,[1]);
    finishNew(); await Promise.resolve(); assert.equal(scheduler.pending(),0);
  }
});
