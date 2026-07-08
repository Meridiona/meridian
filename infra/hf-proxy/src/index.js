// Hugging Face caching proxy (Cloudflare Worker) for Meridian model downloads.
//
// A transparent reverse-proxy to huggingface.co. Its whole job is to make the
// on-device model weights download fast and reliably on first run: it caches the
// large weight blobs at Cloudflare's edge (backed by Cache Reserve) so users stop
// stalling/retrying against HF directly. Metadata and everything else is proxied
// through unchanged. On ANY internal error it degrades to a plain pass-through to
// HF origin, so it can never be worse than talking to HF directly.
//
// Enabled app-side by pointing the MLX server's HF_ENDPOINT at this Worker's route
// (mlx_server::hf_endpoint + the mlx-server plist). Ships DISABLED until deployed.

const HF_ORIGIN = 'https://huggingface.co'
const ONE_YEAR_SECS = 31_536_000

/** A weight/file fetch (`/<repo>/resolve/<rev>/<path>`) — the large, cacheable
 *  objects. Metadata (`/api/...`) and other paths are proxied but not force-cached. */
function isResolvePath(pathname) {
  return pathname.includes('/resolve/')
}

/** Copy the client headers for the upstream request, dropping hop-by-hop / routing
 *  headers Cloudflare will set itself. Authorization + Range are preserved so a
 *  user's `hf auth login` token (rate-limit lift) and resume requests still work. */
function forwardHeaders(request) {
  const h = new Headers(request.headers)
  h.delete('host')
  h.delete('cf-connecting-ip')
  h.delete('cf-ipcountry')
  h.delete('x-forwarded-host')
  return h
}

/** Transparent pass-through to HF origin (redirects followed). Used for metadata,
 *  HEAD, resume (Range) GETs, and as the error fallback. */
function passthrough(upstream, request) {
  const init = {
    method: request.method,
    headers: forwardHeaders(request),
    redirect: 'follow',
  }
  if (request.method !== 'GET' && request.method !== 'HEAD') {
    init.body = request.body
  }
  return fetch(upstream, init)
}

export default {
  async fetch(request, _env, _ctx) {
    const url = new URL(request.url)
    const upstream = HF_ORIGIN + url.pathname + url.search

    try {
      const isFullWeightGet =
        request.method === 'GET' &&
        isResolvePath(url.pathname) &&
        !request.headers.get('range') // resume (Range) requests bypass the cache

      if (isFullWeightGet) {
        // Cache the full weight blob at the edge. `cacheEverything` + a STABLE
        // `cacheKey` (origin+pathname, i.e. the signed CDN query stripped) is what
        // makes this work: HF's resolve endpoint 302-redirects to a CDN URL whose
        // signature query varies per request, so without a stable key every pull
        // would miss. Redirects are followed inside this fetch, so the cached entity
        // is the final bytes. Cache Reserve (zone setting) backs objects over the
        // 512 MB edge cap — required for the ~1.4 GB llm file.
        const resp = await fetch(upstream, {
          method: 'GET',
          headers: forwardHeaders(request),
          redirect: 'follow',
          cf: {
            cacheEverything: true,
            cacheTtl: ONE_YEAR_SECS, // weights are immutable per (repo, revision)
            cacheKey: url.origin + url.pathname,
          },
        })
        const out = new Response(resp.body, resp)
        out.headers.set('x-hf-proxy', 'meridian')
        return out
      }

      // Metadata / HEAD / resume / anything else → transparent proxy.
      const resp = await passthrough(upstream, request)
      const out = new Response(resp.body, resp)
      out.headers.set('x-hf-proxy', 'meridian')
      return out
    } catch (err) {
      // Never fail worse than direct HF: fall back to a plain origin pass-through.
      try {
        return await passthrough(upstream, request)
      } catch (_e) {
        return new Response(`hf-proxy upstream error: ${err}`, { status: 502 })
      }
    }
  },
}
