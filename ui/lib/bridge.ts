//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Tauri bridge helpers for the Next-fold transition. The dashboard runs in TWO
// places during the fold: a plain browser (no Tauri) and the Tauri window
// (global __TAURI__ via withGlobalTauri). These helpers let one component serve
// both: prefer the Rust command in the app, fall back to the /api route in the
// browser. The fetch fallback is removed at the export cutover (stage 4), when
// /api routes are deleted and the browser path goes away.

type InvokeFn = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>

type TauriBridge = {
  core: { invoke: InvokeFn }
  window: { getCurrentWindow: () => { close: () => Promise<void> } }
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

/** Dual-path mutation (POST): the Rust write command inside the app, else a
 *  POST to the /api route in a browser. `body` is sent as ONE payload object —
 *  to `invoke` under the `body` key (matching the command's `body:` param) and
 *  as the `fetch` JSON body — so both paths carry one identical snake_case shape.
 *  Returns the freshly-computed response both paths emit; throws on failure so
 *  callers can roll back to server truth. Removed at the export cutover. */
export async function mutate<T = unknown>(
  apiPath: string,
  command: string,
  body: Record<string, unknown>,
): Promise<T> {
  const t = tauri()
  if (t) return t.core.invoke(command, { body }) as Promise<T>
  const r = await fetch(apiPath, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!r.ok) throw new Error(`${apiPath} → ${r.status}`)
  return r.json() as Promise<T>
}
