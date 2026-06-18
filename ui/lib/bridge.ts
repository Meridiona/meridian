//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Tauri bridge helpers for the Next-fold transition. The dashboard runs in TWO
// places during the fold: a plain browser (no Tauri) and the Tauri window
// (global __TAURI__ via withGlobalTauri). These helpers let one component serve
// both: prefer the Rust command in the app, fall back to the /api route in the
// browser. The fetch fallback is removed at the export cutover (stage 4), when
// /api routes are deleted and the browser path goes away.

type InvokeFn = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>
type UnlistenFn = () => void

type TauriBridge = {
  core: { invoke: InvokeFn }
  window: { getCurrentWindow: () => { close: () => Promise<void> } }
  // From withGlobalTauri — the event module (listen returns a promise of the
  // unlisten fn). Used by `subscribe` for the ported SSE streams.
  event: {
    listen: <T>(event: string, handler: (e: { payload: T }) => void) => Promise<UnlistenFn>
  }
}

declare global {
  interface Window {
    __TAURI__?: TauriBridge
  }
}

/** The global Tauri bridge, or undefined in a plain browser. */
export function tauri(): TauriBridge | undefined {
  return typeof window !== 'undefined' ? window.__TAURI__ : undefined
}

export function isTauri(): boolean {
  return !!tauri()
}

/** Invoke a Rust command directly. Throws outside a Tauri window — use for
 *  app-only actions (no /api equivalent), e.g. open_permission_pane. */
export async function invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const t = tauri()
  if (!t) throw new Error(`invoke('${cmd}') requires the Meridian app — the Tauri bridge is unavailable in a plain browser.`)
  return t.core.invoke(cmd, args) as Promise<T>
}

/** Dual-path data load: the Rust command inside the app, else the /api route in
 *  a browser. Both return the same shape (commands are byte-identical ports). */
export async function load<T = unknown>(
  apiPath: string,
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const t = tauri()
  if (t) return t.core.invoke(command, args) as Promise<T>
  // Browser fallback: append args as query params so /api routes receive them.
  const qs = args
    ? new URLSearchParams(
        Object.fromEntries(
          Object.entries(args)
            .filter(([, v]) => v != null)
            .map(([k, v]) => [k, String(v)])
        )
      ).toString()
    : ''
  const r = await fetch(qs ? `${apiPath}?${qs}` : apiPath)
  if (!r.ok) throw new Error(`${apiPath} → ${r.status}`)
  return r.json() as Promise<T>
}

/** Dual-path mutation: the Rust write command inside the app, else an HTTP
 *  request to the /api route in a browser. `body` is sent as ONE payload object —
 *  to `invoke` under the `body` key (matching the command's `body:` param) and
 *  as the request JSON body — so both paths carry one identical shape. `method`
 *  is the browser verb (default POST; use 'PATCH'/'DELETE' for those routes); the
 *  app path ignores it (the command name already encodes the operation). For a
 *  path-param route, bake the id into `apiPath` and also into `body` (the route
 *  reads it from the URL, the command from the body). Returns the response both
 *  paths emit; throws an Error whose message is the server's `error` text (so
 *  callers can surface it), letting them roll back. Removed at the export cutover. */
export async function mutate<T = unknown>(
  apiPath: string,
  command: string,
  body: Record<string, unknown>,
  method: 'POST' | 'PUT' | 'PATCH' | 'DELETE' = 'POST',
): Promise<T> {
  const t = tauri()
  if (t) return t.core.invoke(command, { body }) as Promise<T>
  const r = await fetch(apiPath, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!r.ok) {
    // Surface the route's `{ error }` message (the command path rejects with the
    // same human text), falling back to the status line.
    const e = await r.json().catch(() => ({}))
    throw new Error((e as { error?: string }).error ?? `${apiPath} → ${r.status}`)
  }
  return r.json() as Promise<T>
}

/** Dual-path live subscription: a Tauri event in the app, an SSE `EventSource`
 *  in a browser. Replaces the dashboard's four SSE stream routes.
 *
 *  In the app it primes once via the `command` snapshot read (so the first paint
 *  isn't blank), then listens for `eventName` the tray poll loop emits. In a
 *  browser it opens `EventSource(apiPath)` with a 5 s reconnect backoff (the SSE
 *  payload is the same JSON the event carries). Returns an unsubscribe to call
 *  from a `useEffect` cleanup. The SSE branch is removed at the export cutover.
 *
 *  Pass `command: null` to SKIP the snapshot prime — for a stream whose event
 *  carries deltas, not full snapshots (the Logs tail), where the caller primes
 *  separately and `onData` appends. */
export function subscribe<T = unknown>(
  apiPath: string,
  command: string | null,
  eventName: string,
  onData: (data: T) => void,
  args?: Record<string, unknown>,
): () => void {
  const t = tauri()
  if (t) {
    let cancelled = false
    let unlisten: UnlistenFn | null = null
    // Prime with the current snapshot so the view isn't empty until the next emit
    // (skipped for delta streams, where `command` is null and the caller primes).
    if (command) t.core.invoke(command, args).then((d) => { if (!cancelled) onData(d as T) }).catch(() => {})
    t.event
      .listen<T>(eventName, (e) => { if (!cancelled) onData(e.payload) })
      .then((un) => { if (cancelled) un(); else unlisten = un })
      .catch(() => {})
    return () => { cancelled = true; if (unlisten) unlisten() }
  }

  // Browser: SSE with reconnect (mirrors the consumers' previous EventSource use).
  let es: EventSource | null = null
  let retry: ReturnType<typeof setTimeout> | null = null
  let closed = false
  const connect = () => {
    if (closed) return
    es = new EventSource(apiPath)
    es.onmessage = (ev) => { try { onData(JSON.parse(ev.data) as T) } catch { /* ignore */ } }
    es.onerror = () => { es?.close(); es = null; retry = setTimeout(connect, 5_000) }
  }
  connect()
  return () => { closed = true; if (retry) clearTimeout(retry); es?.close() }
}
