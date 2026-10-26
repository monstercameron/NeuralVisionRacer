// Cache validation stays mandatory for existing entries. A cold miss needs only
// the GET, whose response supplies the validator for the next visit.
export function createAssetCache(metrics, {fetcher = globalThis.fetch, cacheStorage = globalThis.caches} = {}) {
  const name = "splat-racer-assets-v1", validatorHeader = "x-splat-validator";
  let opened;
  const store = () => opened ??= (cacheStorage
    ? Promise.resolve().then(() => { metrics.cache_opens++; return cacheStorage.open(name); }).catch(() => null)
    : Promise.resolve(null));
  const validator = response => response?.ok
    ? response.headers.get("etag") || response.headers.get("last-modified") || response.headers.get("content-length")
    : null;
  async function bytes(url, onProgress = () => {}) {
    const cache = await store();
    const cached = cache && await cache.match(url).catch(() => null);
    if (cached) {
      metrics.validator_requests++;
      const head = await fetcher(url, {method: "HEAD", cache: "no-store"}).catch(() => null);
      const current = validator(head);
      if (current && cached.headers.get(validatorHeader) === current) {
        try {
          const data = new Uint8Array(await cached.arrayBuffer());
          metrics.cache_hits++; metrics.cache_bytes += data.length;
          onProgress(data.length, data.length, true);
          return data;
        } catch (_) { /* An unreadable cache entry falls through to a fresh GET. */ }
      }
    }
    metrics.network_requests++;
    const response = await fetcher(url, {cache: "no-store"});
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    const total = Number(response.headers.get("content-length")) || 0;
    let data;
    if (response.body) {
      const reader = response.body.getReader(), parts = [];
      let loaded = 0;
      for (;;) {
        const {done,value} = await reader.read();
        if (done) break;
        parts.push(value); loaded += value.length; metrics.network_bytes += value.length;
        onProgress(loaded,total || loaded,false);
      }
      data = new Uint8Array(loaded);
      let at = 0;
      for (const part of parts) { data.set(part,at); at += part.length; }
    } else {
      data = new Uint8Array(await response.arrayBuffer());
      metrics.network_bytes += data.length; onProgress(data.length,data.length,false);
    }
    const current = validator(response);
    if (cache && current) {
      const headers = {[validatorHeader]:current,"content-type":response.headers.get("content-type") || "application/octet-stream"};
      // Quota/private-mode failures affect only later cache hits, never this load.
      cache.put(url,new Response(data,{headers})).catch(() => {});
    }
    return data;
  }
  async function clear() {
    opened = undefined;
    return cacheStorage ? cacheStorage.delete(name) : false;
  }
  return {bytes,clear};
}
