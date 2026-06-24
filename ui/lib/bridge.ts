//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Tauri bridge helpers. Post-fold the dashboard runs ONLY inside the Tauri
// window (the static Next export rendered in the webview; no Node server, no
// browser path), so every helper reaches Rust directly: `load`/`mutate` →
// `invoke`, `subscribe` → the event bus. The `apiPath` argument is now vestigial
// — it documents which former `/api` route each call replaced (the routes are
// deleted) and is only surfaced in error messages.

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

/** Data load via the Rust command (`apiPath` is the former route it replaced). */
export async function load<T = unknown>(
  apiPath: string,
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const t = tauri()
  if (!t) throw new Error(`load('${apiPath}') requires the Meridian app — Tauri bridge unavailable.`)
  return t.core.invoke(command, args) as Promise<T>
}

/** Mutation via the Rust write command: `body` is passed under the `body` key
 *  (matching the command's `body:` param). The command rejects with a human
 *  `String` error, which propagates as an Error message callers can surface to
 *  roll back. `apiPath`/`method` are vestigial (the former route + verb). */
export async function mutate<T = unknown>(
  apiPath: string,
  command: string,
  body: Record<string, unknown>,
  _method: 'POST' | 'PUT' | 'PATCH' | 'DELETE' = 'POST',
): Promise<T> {
  const t = tauri()
  if (!t) throw new Error(`mutate('${apiPath}') requires the Meridian app — Tauri bridge unavailable.`)
  return t.core.invoke(command, { body }) as Promise<T>
}

/** Live subscription via the Tauri event bus — replaces the four SSE streams.
 *  Primes once via the `command` snapshot read (so the first paint isn't blank),
 *  then listens for `eventName` the tray poll loop emits. Returns an unsubscribe
 *  for a `useEffect` cleanup. Pass `command: null` to SKIP the prime — for a
 *  stream whose event carries deltas, not snapshots (the Logs tail), where the
 *  caller primes separately and `onData` appends. `apiPath` is the former route. */
export function subscribe<T = unknown>(
  apiPath: string,
  command: string | null,
  eventName: string,
  onData: (data: T) => void,
  args?: Record<string, unknown>,
): () => void {
  const t = tauri()
  if (!t) {
    console.warn(`subscribe('${apiPath}') requires the Meridian app — Tauri bridge unavailable.`)
    return () => {}
  }
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
