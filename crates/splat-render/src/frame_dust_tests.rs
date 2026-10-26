use super::*;
use crate::dust::DustParticle;
use std::collections::VecDeque;
#[test]
fn soft_dust_respects_scene_depth_disables_restores_fades_and_resizes() {
    if crate::try_device().is_none(){return;}
    let instance=wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut frame=pollster::block_on(Frame::new(&instance,None,(128,128),false,None)).unwrap();
    frame.set_level(&[crate::shadow_gpu_tests::floor()]).unwrap();frame.profile_sample_every(1);
    let view=View{model:camera::IDENTITY,view:camera::look_at([0.0,5.0,0.01],[0.0;3],[0.0,0.0,-1.0]),
        projection:camera::perspective(65f32.to_radians(),1.0,0.01,100.0),width:128,height:128,near:0.01,far:100.0};
    let base=pollster::block_on(frame.read_rgba(&view));
    let p=DustParticle{origin:[0.0,0.5,0.0],velocity:[0.0;3],normal:[0.0,1.0,0.0],colour:[0.70,0.58,0.43],
        born:0.0,life:2.0,size:0.5,opacity:0.8,seed:0.41};
    let particles=VecDeque::from([p]);frame.set_dust(1,&particles,0.5,1.0);
    let dust=pollster::block_on(frame.read_rgba(&view));frame.wait_idle();
    let changed=base.chunks_exact(4).zip(dust.chunks_exact(4)).filter(|(a,b)|a[..3]!=b[..3]).count();
    assert!(changed>50,"dust should visibly composite onto the scene: {changed}");
    assert!(base.chunks_exact(4).zip(dust.chunks_exact(4)).all(|(a,b)|a[3]==b[3]));
    frame.set_dust(1,&particles,0.5,0.0);assert_eq!(base,pollster::block_on(frame.read_rgba(&view)));
    frame.set_dust(1,&particles,0.5,1.0);assert_eq!(dust,pollster::block_on(frame.read_rgba(&view)),"off/on at a paused tick must restore particles");
    frame.set_dust(1,&particles,3.0,1.0);assert_eq!(base,pollster::block_on(frame.read_rgba(&view)),"expired particles disappear");
    let behind=VecDeque::from([DustParticle{origin:[0.0,-1.0,0.0],..p}]);
    frame.set_dust(2,&behind,0.5,1.0);assert_eq!(base,pollster::block_on(frame.read_rgba(&view)),"ground core depth must occlude dust even with DOF off");
    if frame.gpu_timer.is_some() {assert!(frame.profile_json()["shader_catalogue"].as_array().unwrap().iter().any(|p|p["id"]=="dust"));}
    frame.resize(96,96);frame.set_dust(1,&particles,0.5,1.0);
    assert_eq!(pollster::block_on(frame.read_rgba(&View{width:96,height:96,..view})).len(),96*96*4);
}
#[test]
fn car_core_depth_occludes_particles_behind_the_body() {
    if crate::try_device().is_none(){return;}
    let instance=wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let car=crate::shadow_gpu_tests::floor();
    let mut frame=pollster::block_on(Frame::new(&instance,None,(64,64),false,Some(&car))).unwrap();
    let mut model=camera::IDENTITY;model[13]=0.8;
    let view=View{model,view:camera::look_at([0.0,5.0,0.01],[0.0;3],[0.0,0.0,-1.0]),
        projection:camera::perspective(65f32.to_radians(),1.0,0.01,100.0),width:64,height:64,near:0.01,far:100.0};
    let base=pollster::block_on(frame.read_rgba(&view));
    let p=DustParticle{origin:[0.0,0.0,0.0],velocity:[0.0;3],normal:[0.0,1.0,0.0],colour:[0.7,0.5,0.3],born:0.0,life:2.0,size:0.2,opacity:0.8,seed:0.2};
    frame.set_dust(1,&VecDeque::from([p]),0.5,1.0);
    let result=pollster::block_on(frame.read_rgba(&view));
    let changed:Vec<_>=base.chunks_exact(4).zip(result.chunks_exact(4)).enumerate().filter(|(_, (a,b))|a!=b).map(|(i,(a,b))|(i%64,i/64,a.to_vec(),b.to_vec())).collect();
    assert!(changed.is_empty(),"occlusion differences: {} {:?}",changed.len(),&changed[..changed.len().min(12)]);
}
