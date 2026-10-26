// Browser WebGPU pixel regression fixture for production post.wgsl against a saved source preimage.
const output=document.getElementById("result"), status=document.getElementById("status");
async function run() {
  const adapter=await navigator.gpu.requestAdapter();
  const device=await adapter.requestDevice();
  const errors=[]; device.addEventListener("uncapturederror",e=>errors.push(e.error.message));
  const size=128, format="rgba8unorm-srgb";
  const source=device.createTexture({size:[size,size],format,usage:GPUTextureUsage.TEXTURE_BINDING|GPUTextureUsage.COPY_DST});
  const pixels=new Uint8Array(size*size*4);
  for(let y=0;y<size;y++) for(let x=0;x<size;x++) {
    const i=(y*size+x)*4;
    pixels.set(y<64 ? [x*2,x*2,x*2,255] : [x*2,(x*11+y*3)%256,(x*7+y*13)%256,255],i);
  }
  device.queue.writeTexture({texture:source},pixels,{bytesPerRow:size*4},[size,size]);
  const depth=()=>device.createTexture({size:[size,size],format:"depth32float",usage:GPUTextureUsage.RENDER_ATTACHMENT|GPUTextureUsage.TEXTURE_BINDING});
  const car=depth(), level=depth();
  const uniform=device.createBuffer({size:192,usage:GPUBufferUsage.UNIFORM|GPUBufferUsage.COPY_DST});
  const target=device.createTexture({size:[size,size],format,usage:GPUTextureUsage.RENDER_ATTACHMENT|GPUTextureUsage.COPY_SRC});
  const sampler=device.createSampler({minFilter:"linear",magFilter:"linear"});
  const shaderPaths=["../logs/browser-benchmark/post-baseline.wgsl","../crates/splat-render/src/shaders/post.wgsl"];
  const shaders=await Promise.all(shaderPaths.map(async path=>{const r=await fetch(path,{cache:"no-store"});if(!r.ok)throw new Error(`${path}: ${r.status}`);return r.text();}));
  const pipelines=shaders.map(code=>{
    const module=device.createShaderModule({code});
    const pipeline=device.createRenderPipeline({layout:"auto",vertex:{module,entryPoint:"vs_main"},fragment:{module,entryPoint:"fs_main",targets:[{format}]}});
    const group=device.createBindGroup({layout:pipeline.getBindGroupLayout(0),entries:[
      {binding:0,resource:source.createView()},{binding:1,resource:sampler},{binding:2,resource:{buffer:uniform}},
      {binding:3,resource:car.createView()},{binding:4,resource:level.createView()}]});
    return {pipeline,group};
  });
  async function render(which,carDepth,values) {
    device.queue.writeBuffer(uniform,0,values);
    const encoder=device.createCommandEncoder();
    for (const [texture,clear] of [[car,carDepth],[level,0.6]]) {
      const pass=encoder.beginRenderPass({colorAttachments:[],depthStencilAttachment:{view:texture.createView(),depthClearValue:clear,depthLoadOp:"clear",depthStoreOp:"store"}}); pass.end();
    }
    const pass=encoder.beginRenderPass({colorAttachments:[{view:target.createView(),clearValue:{r:0,g:0,b:0,a:1},loadOp:"clear",storeOp:"store"}]});
    pass.setPipeline(pipelines[which].pipeline);pass.setBindGroup(0,pipelines[which].group);pass.draw(3);pass.end();
    const read=device.createBuffer({size:size*size*4,usage:GPUBufferUsage.COPY_DST|GPUBufferUsage.MAP_READ});
    encoder.copyTextureToBuffer({texture:target},{buffer:read,bytesPerRow:size*4},[size,size]);
    device.queue.submit([encoder.finish()]);await read.mapAsync(GPUMapMode.READ);
    const data=new Uint8Array(read.getMappedRange()).slice();read.unmap();read.destroy();return data;
  }
  const cases=[];
  for(const [name,grade,clarity,blur,carDepth] of [["background-grade",1,0,0,1],["car-clarity-grade",1,1,0,0.4],
    ["partial-grade",0.5,0,0,1],["grade-off",0,0,0,1],["peripheral-blur",1,0,0.7,1]]) {
    const values=new Float32Array(48);
    values.set([blur,0.001,0,grade,0,1/size,1/size,0.6,0.82,0.96,0.2,0.35,
      -0.045,0.014,0.032,0.14,0.055,0.018,-0.035,0,clarity*0.85,clarity*0.18,0,0,0.1,0.16,0,0.008]);
    const a=await render(0,carDepth,values),b=await render(1,carDepth,values);
    let max=0,total=0,changed=0;
    for(let i=0;i<a.length;i++){const delta=Math.abs(a[i]-b[i]);max=Math.max(max,delta);total+=delta;if(delta)changed++;}
    cases.push({name,max_channel_delta:max,mean_channel_delta:total/a.length,changed_channels:changed,channels:a.length,passed:max<=1});
  }
  const report={cases,errors,passed:cases.every(c=>c.passed)&&errors.length===0};
  output.textContent=JSON.stringify(report,null,2);output.dataset.complete="true";
  status.textContent=report.passed?"Passed: all differences within one 8-bit code value.":"FAILED";
  device.destroy();
}
run().catch(e=>{status.textContent="FAILED";output.textContent=e.stack||String(e);output.dataset.error="true";});
