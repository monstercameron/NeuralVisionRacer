// Browser-only comparison of the production DOF shader and its retained preimage.
// Pixel readback is outside the timed render pass. Samples alternate shader order.
const output = document.getElementById("result"), status = document.getElementById("status");
async function run() {
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) throw new Error("WebGPU adapter unavailable");
  const timestamps = adapter.features.has("timestamp-query");
  const device = await adapter.requestDevice({requiredFeatures: timestamps ? ["timestamp-query"] : []});
  const errors = []; device.addEventListener("uncapturederror", e => errors.push(e.error.message));
  const width = 1280, height = 720, format = "rgba8unorm-srgb";
  const source = device.createTexture({size:[width,height],format,usage:GPUTextureUsage.TEXTURE_BINDING|GPUTextureUsage.COPY_DST});
  const pixels = new Uint8Array(width*height*4);
  for(let y=0;y<height;y++) for(let x=0;x<width;x++) {
    const i=(y*width+x)*4;
    pixels.set([(x*7+y*3)%256,(x*3+y*11)%256,(x*13+y*7)%256,255],i);
  }
  device.queue.writeTexture({texture:source},pixels,{bytesPerRow:width*4},[width,height]);
  const depth = () => device.createTexture({size:[width,height],format:"depth32float",usage:GPUTextureUsage.RENDER_ATTACHMENT|GPUTextureUsage.TEXTURE_BINDING});
  const car = depth(), level = depth();
  // Foreground strip, thin objects and sharply varying background depth exercise
  // tap rejection and the grown car silhouette, including texture boundaries.
  const depthModule = device.createShaderModule({code:`
    @vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4f {
      let uv=vec2f(f32((i<<1u)&2u),f32(i&2u));return vec4f(uv*2.0-1.0,0.0,1.0);
    }
    @fragment fn fs(@builtin(position) p:vec4f)->@builtin(frag_depth) f32 {
      let z=select(select(60.0,18.0,(u32(p.x)/13u+u32(p.y)/17u)%2u==0u),3.0,p.x<90.0 || (p.x>760.0 && p.x<764.0));
      return 1.0001-0.10001/z;
    }
    @fragment fn car(@builtin(position) p:vec4f)->@builtin(frag_depth) f32 {
      return select(1.0,1.0001-0.10001/5.0,p.x>450.0 && p.x<730.0 && p.y>290.0 && p.y<650.0);
    }`});
  const depthPipeline = name => device.createRenderPipeline({layout:"auto",vertex:{module:depthModule,entryPoint:"vs"},fragment:{module:depthModule,entryPoint:name,targets:[]},depthStencil:{format:"depth32float",depthWriteEnabled:true,depthCompare:"always"}});
  const depthEncoder = device.createCommandEncoder();
  for(const [texture,name] of [[level,"fs"],[car,"car"]]) {
    const pass=depthEncoder.beginRenderPass({colorAttachments:[],depthStencilAttachment:{view:texture.createView(),depthClearValue:1,depthLoadOp:"clear",depthStoreOp:"store"}});
    pass.setPipeline(depthPipeline(name));pass.draw(3);pass.end();
  }
  device.queue.submit([depthEncoder.finish()]);
  const uniform=device.createBuffer({size:32,usage:GPUBufferUsage.UNIFORM|GPUBufferUsage.COPY_DST});
  const target=device.createTexture({size:[width,height],format,usage:GPUTextureUsage.RENDER_ATTACHMENT|GPUTextureUsage.COPY_SRC});
  const sampler=device.createSampler({minFilter:"linear",magFilter:"linear"});
  const paths=["../logs/depth-prepass-reference/dof.wgsl","../crates/splat-render/src/shaders/dof.wgsl"];
  const shaders=await Promise.all(paths.map(async path=>{const response=await fetch(path,{cache:"no-store"});if(!response.ok)throw new Error(path);return response.text();}));
  const pipelines=shaders.map(code=>{
    const module=device.createShaderModule({code});
    const pipeline=device.createRenderPipeline({layout:"auto",vertex:{module,entryPoint:"vs_main"},fragment:{module,entryPoint:"fs_main",targets:[{format}]}});
    const group=device.createBindGroup({layout:pipeline.getBindGroupLayout(0),entries:[{binding:0,resource:source.createView()},{binding:1,resource:sampler},{binding:2,resource:level.createView()},{binding:3,resource:car.createView()},{binding:4,resource:{buffer:uniform}}]});
    return {pipeline,group};
  });
  const queries=timestamps ? device.createQuerySet({type:"timestamp",count:2}) : null;
  const resolved=timestamps ? device.createBuffer({size:16,usage:GPUBufferUsage.QUERY_RESOLVE|GPUBufferUsage.COPY_SRC}) : null;
  const timings=timestamps ? device.createBuffer({size:16,usage:GPUBufferUsage.COPY_DST|GPUBufferUsage.MAP_READ}) : null;
  const read=device.createBuffer({size:width*height*4,usage:GPUBufferUsage.COPY_DST|GPUBufferUsage.MAP_READ});
  async function render(which, capture=false) {
    const encoder=device.createCommandEncoder();
    const pass=encoder.beginRenderPass({colorAttachments:[{view:target.createView(),loadOp:"clear",storeOp:"store",clearValue:{r:0,g:0,b:0,a:1}}],...(timestamps?{timestampWrites:{querySet:queries,beginningOfPassWriteIndex:0,endOfPassWriteIndex:1}}:{})});
    pass.setPipeline(pipelines[which].pipeline);pass.setBindGroup(0,pipelines[which].group);pass.draw(3);pass.end();
    if(timestamps){encoder.resolveQuerySet(queries,0,2,resolved,0);encoder.copyBufferToBuffer(resolved,0,timings,0,16);}
    if(capture)encoder.copyTextureToBuffer({texture:target},{buffer:read,bytesPerRow:width*4},[width,height]);
    device.queue.submit([encoder.finish()]);
    let ms=null, data=null;
    if(timestamps){await timings.mapAsync(GPUMapMode.READ);const values=new BigUint64Array(timings.getMappedRange());ms=Number(values[1]-values[0])/1e6;timings.unmap();}
    else await device.queue.onSubmittedWorkDone();
    if(capture){await read.mapAsync(GPUMapMode.READ);data=new Uint8Array(read.getMappedRange()).slice();read.unmap();}
    return {ms,data};
  }
  const cases=[];
  for(const [name,strength,focus,range,radius] of [["disabled",0,8,25,8],["sharp",1,1e6,25,8],["mixed-depth",1,8,25,8],["wide-blur",1,1,12,18]]) {
    status.textContent=`Comparing ${name}`;
    device.queue.writeBuffer(uniform,0,new Float32Array([strength,focus,range,radius,-1.0001,-0.10001,1/width,1/height]));
    const a=(await render(0,true)).data,b=(await render(1,true)).data;
    let max=0,changed=0,total=0;
    for(let i=0;i<a.length;i++){const delta=Math.abs(a[i]-b[i]);max=Math.max(max,delta);total+=delta;if(delta)changed++;}
    for(let i=0;i<4;i++){await render(0);await render(1);}
    const samples=[[],[]];
    for(let i=0;i<30;i++) for(const which of (i%2 ? [1,0] : [0,1])) {const result=await render(which);if(result.ms!==null)samples[which].push(result.ms);}
    const summary=values=>{const sorted=[...values].sort((a,b)=>a-b);return {p50:sorted[Math.floor(sorted.length/2)]??null,p95:sorted[Math.floor(sorted.length*.95)]??null,samples_ms:values};};
    cases.push({name,max_channel_delta:max,changed_channels:changed,mean_channel_delta:total/a.length,passed:max<=1,baseline:summary(samples[0]),candidate:summary(samples[1])});
  }
  const report={dimensions:[width,height],timestamp_queries:timestamps,cases,errors,passed:errors.length===0&&cases.every(c=>c.passed)};
  output.textContent=JSON.stringify(report,null,2);output.dataset.complete="true";status.textContent=report.passed?"Passed pixel comparisons. Timings below.":"FAILED pixel comparison";
  device.destroy();
}
run().catch(error=>{status.textContent="FAILED";output.textContent=error.stack||String(error);output.dataset.error="true";});
