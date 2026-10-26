import test from 'node:test';
import assert from 'node:assert/strict';
import {createAssetCache} from '../web/asset-cache.js';
const metrics = () => ({cache_opens:0,validator_requests:0,network_requests:0,cache_hits:0,network_bytes:0,cache_bytes:0});
function fixture({etag='one',failOpen=false,failPut=false,failMatch=false,failHead=false,noValidator=false}={}) {
  const calls=[], entries=new Map(); let opens=0, version=etag;
  const body=Uint8Array.of(3,1,4,1,5);
  const cache={
    async match(url) {if(failMatch)throw Error('cache I/O');return entries.get(url)?.clone();},
    async put(url,response) {if(failPut)throw Error('quota');entries.set(url,response.clone());},
  };
  const storage={async open(){opens++;if(failOpen)throw Error('private mode');return cache;},async delete(){entries.clear();return true;}};
  const fetcher=async(url,options)=>{
    const method=options.method||'GET';calls.push([url,method]);
    if(method==='HEAD'&&failHead)throw Error('offline validator');
    return new Response(method==='HEAD'?null:body,{headers:noValidator?{}:{etag:version,'content-length':String(body.length)}});
  };
  const stats=metrics(), loader=createAssetCache(stats,{fetcher,cacheStorage:storage});
  return {loader,calls,stats,entries,body,opens:()=>opens,version:value=>version=value};
}
test('cold assets use one GET each and share one cache open across concurrent loads',async()=>{
  const f=fixture();const all=await Promise.all(Array.from({length:8},(_,i)=>f.loader.bytes(`/asset-${i}`)));
  assert.equal(f.opens(),1);assert.equal(f.calls.length,8);assert(f.calls.every(c=>c[1]==='GET'));
  all.forEach(bytes=>assert.deepEqual(bytes,f.body));assert.equal(f.stats.network_bytes,40);
});
test('warm assets validate by HEAD and reuse the full exact cached payload',async()=>{
  const f=fixture();await f.loader.bytes('/asset');let progress;
  assert.deepEqual(await f.loader.bytes('/asset',(...args)=>progress=args),f.body);
  assert.deepEqual(f.calls.map(c=>c[1]),['GET','HEAD']);assert.deepEqual(progress,[5,5,true]);
  assert.equal(f.stats.cache_hits,1);assert.equal(f.stats.cache_bytes,5);
});
test('changed validator fetches and stores a fresh version',async()=>{
  const f=fixture();await f.loader.bytes('/asset');f.version('two');await f.loader.bytes('/asset');await f.loader.bytes('/asset');
  assert.deepEqual(f.calls.map(c=>c[1]),['GET','HEAD','GET','HEAD']);
  assert.equal(f.entries.get('/asset').headers.get('x-splat-validator'),'two');
});
for(const failure of ['failOpen','failPut','failMatch','failHead'])test(`${failure} preserves successful network loading`,async()=>{
  const f=fixture({[failure]:true});assert.deepEqual(await f.loader.bytes('/asset'),f.body);assert.deepEqual(await f.loader.bytes('/asset'),f.body);
});
test('responses without validators are downloaded directly rather than trusted as current',async()=>{
  const f=fixture({noValidator:true});await f.loader.bytes('/asset');await f.loader.bytes('/asset');
  assert.deepEqual(f.calls.map(c=>c[1]),['GET','GET']);assert.equal(f.stats.cache_hits,0);
});
test('clear invalidates the reused handle and the next request opens the cache again',async()=>{
  const f=fixture();await f.loader.bytes('/asset');assert.equal(await f.loader.clear(),true);await f.loader.bytes('/asset');
  assert.equal(f.opens(),2);assert.deepEqual(f.calls.map(c=>c[1]),['GET','GET']);
});
test('unavailable Cache Storage still downloads bytes and reports progress',async()=>{
  const stats=metrics();const loader=createAssetCache(stats,{cacheStorage:null,fetcher:async()=>new Response(Uint8Array.of(7,8))});
  let final;assert.deepEqual(await loader.bytes('/asset',(...args)=>final=args),Uint8Array.of(7,8));assert.deepEqual(final,[2,2,false]);assert.equal(await loader.clear(),false);
});
test('HTTP errors retain the failing asset and response status',async()=>{
  const loader=createAssetCache(metrics(),{cacheStorage:null,fetcher:async()=>new Response(null,{status:404})});
  await assert.rejects(loader.bytes('/missing'),/\/missing: HTTP 404/);
});
test('cache uses the GET validator when a resource changes between validation and download',async()=>{
  const entries=new Map([['/asset',new Response(Uint8Array.of(1),{headers:{'x-splat-validator':'old'}})]]);
  const cache={async match(url){return entries.get(url)?.clone();},async put(url,response){entries.set(url,response.clone());}};
  const fetcher=async(url,options)=>options.method==='HEAD'
    ?new Response(null,{headers:{etag:'intermediate'}})
    :new Response(Uint8Array.of(2),{headers:{etag:'newest'}});
  const loader=createAssetCache(metrics(),{cacheStorage:{async open(){return cache;}},fetcher});
  assert.deepEqual(await loader.bytes('/asset'),Uint8Array.of(2));
  assert.equal(entries.get('/asset').headers.get('x-splat-validator'),'newest');
});
for(const header of ['last-modified','content-length'])test(`warm cache validates with ${header} fallback`,async()=>{
  const entries=new Map(),calls=[];
  const cache={async match(url){return entries.get(url)?.clone();},async put(url,response){entries.set(url,response.clone());}};
  const loader=createAssetCache(metrics(),{cacheStorage:{async open(){return cache;}},fetcher:async(url,options)=>{
    calls.push(options.method||'GET');return new Response(options.method==='HEAD'?null:Uint8Array.of(1,2),{headers:{[header]:header==='content-length'?'2':'Mon, 26 Oct 2026 16:00:00 GMT'}});
  }});
  await loader.bytes('/asset');assert.deepEqual(await loader.bytes('/asset'),Uint8Array.of(1,2));assert.deepEqual(calls,['GET','HEAD']);
});
