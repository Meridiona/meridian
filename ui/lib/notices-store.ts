//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Module-level singleton that owns the SSE broadcast state. Node.js module
// cache keeps this alive for the lifetime of the process — one shared
// setInterval, one Set of open SSE controllers. Adding more browser tabs adds
// controllers to the Set but does NOT create more DB queries or more timers.
//
// State is stored on globalThis so Next.js HMR module re-evaluation doesn't
// spawn a second broadcast loop while the first keeps running.

import getDb from '@/lib/db'

export interface Notice {
  notice_id: string
  severity: 'error' | 'warning'
  title: string
  detail: string
  remedy: string | null
  raised_at: string
}

type Controller = ReadableStreamDefaultController<Uint8Array>

// Store all mutable singleton state on globalThis to survive HMR re-evaluation.
const g = globalThis as typeof globalThis & {
  _meridianNoticesControllers?: Set<Controller>
  _meridianNoticesInterval?: ReturnType<typeof setInterval>
  _meridianNoticesSnapshot?: string
}
if (!g._meridianNoticesControllers) g._meridianNoticesControllers = new Set()

const encoder = new TextEncoder()

function controllers(): Set<Controller> { return g._meridianNoticesControllers! }
function getSnapshot(): string { return g._meridianNoticesSnapshot ?? '' }
function setSnapshot(s: string) { g._meridianNoticesSnapshot = s }

function queryNotices(): Notice[] {
  try {
    const db = getDb()
    return db
      .prepare(
        'SELECT notice_id, severity, title, detail, remedy, raised_at FROM system_notices ORDER BY raised_at DESC',
      )
      .all() as Notice[]
  } catch {
    return []
  }
}

function broadcast() {
  const ctrls = controllers()
  if (ctrls.size === 0) return
  const notices = queryNotices()
  const snapshot = JSON.stringify(notices)
  if (snapshot === getSnapshot()) return
  setSnapshot(snapshot)
  const payload = encoder.encode(`data: ${snapshot}\n\n`)
  for (const ctrl of ctrls) {
    try {
      ctrl.enqueue(payload)
    } catch {
      // Controller closed — remove it
      ctrls.delete(ctrl)
    }
  }
}

function ensureInterval() {
  if (g._meridianNoticesInterval != null) return
  // One shared interval — 30s is right for error banners (they persist until fixed)
  g._meridianNoticesInterval = setInterval(broadcast, 30_000)
}

export function subscribe(ctrl: Controller): void {
  controllers().add(ctrl)
  ensureInterval()
  // Send current state immediately on connect so the browser doesn't wait 30s.
  // Set lastSnapshot BEFORE enqueueing so a concurrent refresh() sees the
  // up-to-date snapshot and doesn't overwrite it with a stale value.
  const notices = queryNotices()
  const snapshot = JSON.stringify(notices)
  setSnapshot(snapshot)
  ctrl.enqueue(encoder.encode(`data: ${snapshot}\n\n`))
}

export function unsubscribe(ctrl: Controller): void {
  controllers().delete(ctrl)
}

// Force an immediate re-read and broadcast — call after writing to system_notices
// from an API route (e.g. after clearing a notice on successful auth).
export function refresh(): void {
  setSnapshot('') // invalidate snapshot so broadcast always pushes
  broadcast()
}
