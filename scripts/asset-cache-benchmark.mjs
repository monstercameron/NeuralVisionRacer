// Paired cold-cache request counts against the retained repository baseline.
// Uses the actual local HTTP server and byte hashes, not simulated network delays.
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {writeFile} from 'node:fs/promises';
import {createAssetCache} from '../web/asset-cache.js';
const root='http://localhost:8800/';
const paths=['meta.json','means_l.webp','means_u.webp','quats.webp','scales.webp','sh0.webp','shN_labels.webp','shN_centroids.webp'].map(x=>`${root}levels/saburi-alley-yazd/1_0/${x}`);
const src=execFileSync('git',['show','6346b7b:web/main.js'],{encoding:'utf8'});
const old=src.slice(src.indexOf('async function assetBytes('),src.indexOf('/// Empties the asset cache'));
function context() {
  const entries=new Map(), calls=[];
  const store={async match(url){return entries.get(url)?.clone();},async put(url,r){entries.set(url,r.clone());}};
  const caches={async open(){return store;},async delete(){entries.clear();return true;}};
  const fetcher=async(url,options)=>{calls.push(options.method||'GET');return fetch(url,options);};
  const metrics={cache_opens:0,validator_requests:0,network_requests:0,cache_hits:0,network_bytes:0,cache_bytes:0};
  return {caches,fetcher,metrics,calls};
}
const samples=[];
for(let round=0;round<4;round++)for(const variant of round%2?['candidate','baseline']:['baseline','candidate']) {
  const ctx=context();
  const bytes=variant==='baseline'
    ?new Function('caches','fetch','assetMetrics',`const CACHE_NAME='splat-racer-assets-v1',VALIDATOR_HEADER='x-splat-validator'; ${old}; return assetBytes;`)(ctx.caches,ctx.fetcher,ctx.metrics)
    :createAssetCache(ctx.metrics,{fetcher:ctx.fetcher,cacheStorage:ctx.caches}).bytes;
  const started=performance.now();const outputs=await Promise.all(paths.map(p=>bytes(p)));
  const cold_ms=performance.now()-started, cold_methods=[...ctx.calls];
  const hashes=outputs.map(b=>createHash('sha256').update(b).digest('hex'));
  ctx.calls.length=0;const warm=await Promise.all(paths.map(p=>bytes(p)));
  if(JSON.stringify(hashes)!==JSON.stringify(warm.map(b=>createHash('sha256').update(b).digest('hex'))))throw Error('Payload mismatch on cache hit');
  samples.push({round,variant,cold_ms,cold_methods,warm_methods:ctx.calls,hashes,bytes:outputs.reduce((n,b)=>n+b.length,0)});
}
const baseline=samples.find(s=>s.variant==='baseline').hashes;
if(samples.some(s=>JSON.stringify(s.hashes)!==JSON.stringify(baseline)))throw Error('Baseline/candidate payload mismatch');
const report={assets:paths,scope:'local HTTP; fresh per-round Cache Storage fixture; warm filesystem; alternating order; no artificial latency',samples};
if(process.argv[2])await writeFile(process.argv[2],JSON.stringify(report,null,2));
for(const s of samples)console.log(JSON.stringify({round:s.round,variant:s.variant,cold_ms:s.cold_ms,requests:s.cold_methods.length,head:s.cold_methods.filter(m=>m==='HEAD').length,get:s.cold_methods.filter(m=>m==='GET').length,warm_requests:s.warm_methods.length,bytes:s.bytes}));
