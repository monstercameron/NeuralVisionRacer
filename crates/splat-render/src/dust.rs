//! Bounded soft dust billboards; no per-particle GPU allocation or readback.
use crate::{camera,scene::View,COLOR_FORMAT};
use std::{cell::{Cell,RefCell},collections::VecDeque};
pub const CAPACITY:usize=2048;
const STRIDE:u64=80;
#[derive(Clone,Copy,Debug)]
pub struct DustParticle {
    pub origin:[f32;3],pub velocity:[f32;3],pub normal:[f32;3],pub colour:[f32;3],
    pub born:f32,pub life:f32,pub size:f32,pub opacity:f32,pub seed:f32,
}
impl DustParticle {
    pub fn position(&self,time:f32)->[f32;3] {
        let age=(time-self.born).max(0.0);let drag=(1.0-(-0.65*age).exp())/0.65;
        std::array::from_fn(|i|self.origin[i]+self.velocity[i]*drag+self.normal[i]*0.03*age*age)
    }
    pub fn radius(&self,time:f32)->f32 {self.size*(1.0+2.4*((time-self.born)/self.life).clamp(0.0,1.0))}
}
#[derive(Clone,Copy,Default)]
pub(crate) struct Prepared {pub count:u32,pub rect:Option<[u32;4]>}
pub(crate) struct DustLayer {
    pipeline:wgpu::RenderPipeline,layout:wgpu::BindGroupLayout,uniform:wgpu::Buffer,instances:wgpu::Buffer,group:wgpu::BindGroup,
    particles:Vec<DustParticle>,time:f32,strength:f32,revision:Option<u64>,prepared:Cell<Prepared>,uploaded:Cell<u64>,
    cache:RefCell<Option<(camera::Mat4,u32,u32,u64,u32)>>,metrics:RefCell<crate::metrics::Metrics>,
}
impl DustLayer {
    pub fn new(device:&wgpu::Device,level:&wgpu::TextureView,car:&wgpu::TextureView)->Self {
        let uniform=device.create_buffer(&wgpu::BufferDescriptor{label:Some("dust view"),size:128,usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
        let instances=device.create_buffer(&wgpu::BufferDescriptor{label:Some("dust particles"),size:CAPACITY as u64*STRIDE,usage:wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
        let buffer=|binding,ty|wgpu::BindGroupLayoutEntry{binding,visibility:wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty:wgpu::BindingType::Buffer{ty,has_dynamic_offset:false,min_binding_size:None},count:None};
        let depth=|binding|wgpu::BindGroupLayoutEntry{binding,visibility:wgpu::ShaderStages::FRAGMENT,
            ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Depth,view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None};
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("dust"),entries:&[
            buffer(0,wgpu::BufferBindingType::Uniform),buffer(1,wgpu::BufferBindingType::Storage{read_only:true}),depth(2),depth(3)]});
        let group=bind(device,&layout,&uniform,&instances,level,car);
        let module=crate::shader_module(device,"soft material dust",include_str!("shaders/dust.wgsl"));
        let pl=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:Some("dust"),bind_group_layouts:&[Some(&layout)],immediate_size:0});
        let pipeline=device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{label:Some("dust"),layout:Some(&pl),
            vertex:wgpu::VertexState{module:&module,entry_point:Some("vs_main"),compilation_options:Default::default(),buffers:&[]},
            fragment:Some(wgpu::FragmentState{module:&module,entry_point:Some("fs_main"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState{
                format:COLOR_FORMAT,blend:Some(wgpu::BlendState{color:wgpu::BlendComponent{src_factor:wgpu::BlendFactor::One,dst_factor:wgpu::BlendFactor::OneMinusSrcAlpha,operation:wgpu::BlendOperation::Add},
                    alpha:wgpu::BlendComponent{src_factor:wgpu::BlendFactor::Zero,dst_factor:wgpu::BlendFactor::One,operation:wgpu::BlendOperation::Add}}),write_mask:wgpu::ColorWrites::COLOR})]}),
            primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview_mask:None,cache:None});
        Self{pipeline,layout,uniform,instances,group,particles:Vec::new(),time:0.0,strength:0.0,revision:None,
            prepared:Cell::default(),uploaded:Cell::new(0),cache:RefCell::new(None),metrics:RefCell::default()}
    }
    pub fn rebind(&mut self,device:&wgpu::Device,level:&wgpu::TextureView,car:&wgpu::TextureView) {
        self.group=bind(device,&self.layout,&self.uniform,&self.instances,level,car);self.cache.borrow_mut().take();
    }
    pub fn set(&mut self,revision:u64,particles:&VecDeque<DustParticle>,time:f32,strength:f32) {
        if self.revision!=Some(revision) {self.particles=particles.iter().copied().take(CAPACITY).collect();self.revision=Some(revision);}
        self.time=time;self.strength=strength.clamp(0.0,2.0);
    }
    pub fn prepare(&self,queue:&wgpu::Queue,view:&View,clock:Option<fn()->f64>)->Prepared {
        if self.strength<=0.0||self.particles.is_empty() {self.prepared.set(Prepared::default());self.cache.borrow_mut().take();return Prepared::default();}
        let start=clock.map(|c|c());let vp=camera::mul(&view.projection,&view.view);
        let key=(vp,view.width,view.height,self.revision.unwrap_or(0),self.time.to_bits());
        let inverse=camera::inverse(&view.view).unwrap_or(camera::IDENTITY);
        let right=[inverse[0],inverse[1],inverse[2]];let up=[inverse[4],inverse[5],inverse[6]];
        let mut u=vp.to_vec();u.extend([right[0],right[1],right[2],0.0]);u.extend([up[0],up[1],up[2],0.0]);
        u.extend([self.time,self.strength,view.projection[10],view.projection[14]]);u.extend([view.width as f32,view.height as f32,0.0,0.0]);
        queue.write_buffer(&self.uniform,0,&f32_bytes(&u));
        if self.cache.borrow().as_ref()==Some(&key){return self.prepared.get();}
        let mut visible=Vec::new();let mut union=None;
        for p in &self.particles {
            let age=self.time-p.born;if age<0.0||age>=p.life {continue;}
            let position=p.position(self.time);let eye=camera::transform_point(&view.view,position);
            if -eye[2]<view.near||-eye[2]>90.0 {continue;}
            if let Some(rect)=projected_rect(position,p.radius(self.time),right,up,&vp,view.width,view.height) {
                union=crate::tire::union_rect(union,Some(rect));visible.push((-eye[2],p));
            }
        }
        visible.sort_by(|a,b|b.0.total_cmp(&a.0));
        let mut data=Vec::with_capacity(visible.len()*20);
        for (_,p) in &visible {
            data.extend(p.origin);data.push(p.born);data.extend(p.velocity);data.push(p.life);
            data.extend(p.colour.map(|v|if v<=0.04045 {v/12.92} else {((v+0.055)/1.055).powf(2.4)}));data.push(p.opacity);
            data.extend([p.size,p.seed,0.0,0.0]);data.extend(p.normal);data.push(0.0);
        }
        if !data.is_empty(){queue.write_buffer(&self.instances,0,&f32_bytes(&data));}
        self.uploaded.set(self.uploaded.get()+data.len() as u64*4);
        let prepared=Prepared{count:visible.len() as u32,rect:union};self.prepared.set(prepared);*self.cache.borrow_mut()=Some(key);
        if let (Some(c),Some(t))=(clock,start){self.metrics.borrow_mut().record("prepare_cull_sort_upload",c()-t);}
        prepared
    }
    pub fn draw(&self,encoder:&mut wgpu::CommandEncoder,color:&wgpu::TextureView,count:u32,timestamps:Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor{label:Some("material dust"),
            color_attachments:&[Some(wgpu::RenderPassColorAttachment{view:color,depth_slice:None,resolve_target:None,
                ops:wgpu::Operations{load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store}})],timestamp_writes:timestamps,..Default::default()});
        pass.set_pipeline(&self.pipeline);pass.set_bind_group(0,&self.group,&[]);pass.draw(0..6,0..count);
    }
    pub fn strength(&self)->f32 {self.strength}
    pub fn reset_profile(&self) {self.metrics.borrow_mut().reset();self.uploaded.set(0);}
    pub fn profile_json(&self)->serde_json::Value {serde_json::json!({"strength":self.strength,"active":self.particles.len(),"visible":self.prepared.get().count,
        "capacity":CAPACITY,"gpu_instance_bytes":CAPACITY as u64*STRIDE,"uploaded_bytes_total":self.uploaded.get(),"cpu":self.metrics.borrow().json()})}
}
fn bind(device:&wgpu::Device,layout:&wgpu::BindGroupLayout,uniform:&wgpu::Buffer,instances:&wgpu::Buffer,level:&wgpu::TextureView,car:&wgpu::TextureView)->wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("dust"),layout,entries:&[
        wgpu::BindGroupEntry{binding:0,resource:uniform.as_entire_binding()},wgpu::BindGroupEntry{binding:1,resource:instances.as_entire_binding()},
        wgpu::BindGroupEntry{binding:2,resource:wgpu::BindingResource::TextureView(level)},wgpu::BindGroupEntry{binding:3,resource:wgpu::BindingResource::TextureView(car)}]})
}
fn projected_rect(p:[f32;3],radius:f32,right:[f32;3],up:[f32;3],vp:&camera::Mat4,width:u32,height:u32)->Option<[u32;4]> {
    let mut lo=[f32::INFINITY;2];let mut hi=[f32::NEG_INFINITY;2];
    for a in [-1.0,1.0] {for b in [-1.0,1.0] {
        let world:[f32;3]=std::array::from_fn(|i|p[i]+radius*(right[i]*a+up[i]*b));
        let clip:[f32;4]=std::array::from_fn(|i|vp[i]*world[0]+vp[4+i]*world[1]+vp[8+i]*world[2]+vp[12+i]);
        if !clip.iter().all(|v|v.is_finite())||clip[3]<=0.0{return None;}
        let xy=[clip[0]/clip[3],clip[1]/clip[3]];for i in 0..2 {lo[i]=lo[i].min(xy[i]);hi[i]=hi[i].max(xy[i]);}
    }}
    if hi[0]< -1.0||lo[0]>1.0||hi[1]< -1.0||lo[1]>1.0{return None;}
    let x=((lo[0]*0.5+0.5)*width as f32).floor().clamp(0.0,width as f32) as u32;
    let y=((0.5-hi[1]*0.5)*height as f32).floor().clamp(0.0,height as f32) as u32;
    let x1=((hi[0]*0.5+0.5)*width as f32).ceil().clamp(0.0,width as f32) as u32;
    let y1=((0.5-lo[1]*0.5)*height as f32).ceil().clamp(0.0,height as f32) as u32;
    (x1>x&&y1>y).then_some([x,y,x1-x,y1-y])
}

fn f32_bytes(values:&[f32])->Vec<u8> {values.iter().flat_map(|v|v.to_le_bytes()).collect()}
