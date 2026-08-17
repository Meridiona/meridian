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
  core: { invoke: InvokeFn; convertFileSrc: (path: string, protocol?: string) => string }
  window: {
    getCurrentWindow: () => {
      close: () => Promise<void>
      innerSize: () => Promise<{ width: number; height: number }>
      setSize: (size: unknown) => Promise<void>
      // Read by nudgeWindowRepaint to tell the two "bigger than normal" window
      // states apart: native full-screen (where the repaint nudge is the whole
      // point) from merely maximized (where any setSize un-maximizes it).
      isFullscreen: () => Promise<boolean>
      isMaximized: () => Promise<boolean>
    }
    // Constructor for the PhysicalSize argument setSize expects — matches
    // what innerSize() returns, so nudgeWindowRepaint never has to reason
    // about logical/physical DPI conversion.
    PhysicalSize: new (width: number, height: number) => unknown
  }
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

/** Nudge the current window by a 1-physical-pixel resize and back. Works
 *  around a WKWebView/wry compositor bug on macOS: a heavy DOM swap (e.g. a
 *  wizard step change) that happens while the window is already in native
 *  full-screen can leave the webview's drawable pinned to its pre-full-screen
 *  size — content renders correctly within that stale rect, and the rest of
 *  the (genuinely full-screen) NSWindow shows as a black CALayer background
 *  rather than page content. Only a real NSWindow frame-set forces WebKit to
 *  recompute the compositing surface against the window's actual bounds;
 *  confirmed live that exiting full-screen (a real resize) self-heals it.
 *  Two physical pixels, immediately reverted, is enough to trigger that
 *  without a perceptible flash. No-op outside Tauri; failures are swallowed
 *  since this is a cosmetic repaint fix, not something a user action should
 *  ever fail on.
 *
 *  Swallowing failures is not the same as leaking them. Two things this must
 *  never do, both of which leave the window permanently WRONG rather than
 *  merely un-repainted:
 *
 *  1. **Grow and not shrink back.** The grow and the restore are two awaits, and
 *     the old shape put both in one `try` — so a restore that threw left the
 *     window a pixel wider for the rest of the session, silently. The restore
 *     now runs from `finally`, with its own catch.
 *  2. **Restore a size another call is mid-way through changing.** Callers are
 *     step transitions, which can overlap; two concurrent nudges could each read
 *     `innerSize()` while the other had it grown, and then "restore" to that
 *     inflated number. The window creeps a pixel wider every time. Calls are
 *     therefore serialised through one chain, so a nudge always measures a
 *     settled window. */
let repaintQueue: Promise<void> = Promise.resolve()

export function nudgeWindowRepaint(): Promise<void> {
  // `then(run, run)` rather than `.finally(run)`: the next nudge must run whether
  // or not the previous one settled cleanly, and must not inherit its rejection.
  const next = repaintQueue.then(runWindowRepaint, runWindowRepaint)
  repaintQueue = next
  return next
}

async function runWindowRepaint(): Promise<void> {
  const t = tauri()
  if (!t) return
  // Captured as a closure the moment the original size is known, so `finally`
  // can restore without re-reading a size that is currently perturbed.
  let restore: null | (() => Promise<void>) = null
  try {
    const win = t.window.getCurrentWindow()
    const { PhysicalSize } = t.window
    // Never nudge a MAXIMIZED window. The nudge is a real frame-set, and Win32
    // has no way to resize a WS_MAXIMIZE window without clearing the maximized
    // state - so on Windows this fired on every wizard step transition and
    // visibly restored the window down, which is a far worse defect than the
    // stale-drawable repaint it exists to prevent. Full-screen is deliberately
    // NOT excluded: that is the macOS case the nudge was written for, and a
    // macOS window in native full-screen also reports isMaximized() true
    // (tao maps it to `isZoomed`), so the check has to be the pair, not
    // isMaximized alone - otherwise this silently disables the fix it guards.
    const [fullscreen, maximized] = await Promise.all([
      win.isFullscreen().catch(() => false),
      win.isMaximized().catch(() => false),
    ])
    if (maximized && !fullscreen) return
    const { width, height } = await win.innerSize()
    restore = () => win.setSize(new PhysicalSize(width, height))
    await win.setSize(new PhysicalSize(width + 1, height))
  } catch {
    // cosmetic only
  } finally {
    if (restore) {
      try {
        await restore()
      } catch {
        // cosmetic only
      }
    }
  }
}

/** Invoke a Rust command directly. Throws outside a Tauri window — use for
 *  app-only actions (no /api equivalent), e.g. open_permission_pane. */
export async function invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const t = tauri()
  if (!t) throw new Error(`invoke('${cmd}') requires the Meridian app — the Tauri bridge is unavailable in a plain browser.`)
  return t.core.invoke(cmd, args) as Promise<T>
}

/** Forward a UI-side failure to the tray's log, so it lands in the telemetry
 *  spool (and therefore in an Export Diagnostics bundle) rather than only in a
 *  console.
 *
 *  THE DASHBOARD HAS NO CONSOLE ANYONE CAN OPEN. A packaged build ships no
 *  devtools — there is no `devtools` feature or config anywhere in
 *  `tray/src-tauri` — so `console.error` in the webview is written to nothing.
 *  That is not a theoretical gap: the planner's Refresh chip drew `⚠ Sync failed`
 *  on 2026-08-17 while the Jira sync was demonstrably healthy (a hand-run
 *  `meridian tasks-sync` synced 81 issues, `pm_sync_state.last_error` was NULL),
 *  and the reason was unrecoverable — the invoke had been rejected on the JS side,
 *  so there was no Rust span either, and the whole 7-day spool held nothing.
 *
 *  `tray_debug` is the command that already exists for this (it was written for
 *  the popover's `window.onerror`); it logs at INFO with the calling window's
 *  label, which is what makes a report attributable to a surface.
 *
 *  BEST EFFORT, ALWAYS. This is a diagnostic, so it must never turn a handled
 *  failure into a second one: outside Tauri it is a no-op, and a rejected
 *  `tray_debug` is swallowed. */
export function reportUiError(msg: string): void {
  const t = tauri()
  if (!t) return
  // Not `invoke()` above: that one throws outside Tauri, and a diagnostic must
  // not need a try/catch at every call site.
  t.core.invoke('tray_debug', { msg }).catch(() => {})
}

/** True when `href` is an absolute http(s) URL pointing outside `origin` —
 *  i.e. a navigation the Tauri webview cannot perform itself. Exported for
 *  the ExternalLinks interceptor and its tests. */
export function isExternalHttp(href: string, origin: string): boolean {
  if (!/^https?:\/\//i.test(href)) return false
  try { return new URL(href).origin !== origin } catch { return false }
}

/** True when `href` should be handed to the OS instead of navigated in-app:
 *  an external http(s) URL, or a mailto:/tel: link (the WKWebView drops those
 *  just like target="_blank"; both schemes are in the opener plugin's default
 *  scope). The ExternalLinks interceptor's decision function. */
export function opensExternally(href: string, origin: string): boolean {
  return /^(mailto:|tel:)/i.test(href) || isExternalHttp(href, origin)
}

/** Open a URL in the user's default browser / mail client via the
 *  `open_external_url` command (scheme-allowlisted server-side). A plain
 *  `<a target="_blank">` click — or a mailto:/tel: anchor, or `window.open` —
 *  is silently swallowed by the Tauri webview (no new-window handler), so the
 *  ExternalLinks interceptor routes anchors here and "Open ↗" buttons call it
 *  from `onClick`. A command rejection falls back to `window.open` rather than
 *  only logging — a silent miss would reproduce the exact dead-link bug this
 *  helper exists to fix. Outside Tauri (next dev in a plain browser) it uses
 *  `window.open` directly, keeping links alive there too. */
export function openExternal(url: string): void {
  if (!isTauri()) {
    window.open(url, '_blank', 'noopener,noreferrer')
    return
  }
  invoke('open_external_url', { url }).catch((e) => {
    console.warn(`openExternal('${url}') failed:`, e)
    window.open(url, '_blank', 'noopener,noreferrer')
  })
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

/** Turn a local filesystem path into a URL the webview's asset protocol can
 *  load (e.g. `<img src>`), given the path is inside `tauri.conf.json`'s
 *  `security.assetProtocol.scope`. Returns `null` outside Tauri — callers
 *  fall back to whatever they render without a resolved icon/asset. */
export function assetUrl(path: string): string | null {
  const t = tauri()
  if (!t) return null
  return t.core.convertFileSrc(path)
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
  if (command) {
    const prime = () => t.core.invoke(command, args).then((d) => { if (!cancelled) onData(d as T) })
    // Retry once after 2 s on the first failure: the tray can beat the daemon
    // on startup, leaving the DB not yet open. Log on second failure so it is
    // visible in DevTools rather than silently swallowed.
    prime().catch(() => setTimeout(() => { if (!cancelled) prime().catch((e) => console.warn(`subscribe('${apiPath}') snapshot prime failed:`, e)) }, 2000))
  }
  // `drop` is called at most ONCE for a given `un`, on either path below.
  //
  // Tauri's unlisten is not idempotent: it reaches into its own `listeners` map
  // and reads `listeners[eventId].handlerId`, so a second call on an already-
  // removed id throws "undefined is not an object". React runs an effect's
  // cleanup more than once as a matter of course - StrictMode double-invokes it
  // in dev, and any caller that unsubscribes explicitly and then unmounts does
  // it twice for real - so an unsubscribe that is not idempotent will throw
  // eventually. It surfaced as a runtime overlay in dev with no useful stack,
  // pointing into the Tauri shim rather than at the component that caused it.
  const drop = () => { const un = unlisten; unlisten = null; if (un) un() }
  t.event
    .listen<T>(eventName, (e) => { if (!cancelled) onData(e.payload) })
    // Cancelled before the listener even registered: register-then-immediately-
    // remove, since there is no way to withdraw the pending `listen`.
    .then((un) => { unlisten = un; if (cancelled) drop() })
    .catch(() => {})
  return () => { cancelled = true; drop() }
}
