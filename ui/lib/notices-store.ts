//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Module-level singleton that owns the SSE broadcast state. Node.js module
// cache keeps this alive for the lifetime of the process — one shared
// setInterval, one Set of open SSE controllers. Adding more browser tabs adds
// controllers to the Set but does NOT create more DB queries or more timers.

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

const controllers = new Set<Controller>()
const encoder = new TextEncoder()
let lastSnapshot = ''
let intervalStarted = false

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
  const notices = queryNotices()
  const snapshot = JSON.stringify(notices)
  if (snapshot === lastSnapshot) return
  lastSnapshot = snapshot
  const payload = encoder.encode(`data: ${snapshot}\n\n`)
  for (const ctrl of controllers) {
    try {
      ctrl.enqueue(payload)
    } catch {
      // Controller closed — remove it
      controllers.delete(ctrl)
    }
  }
}

function ensureInterval() {
  if (intervalStarted) return
  intervalStarted = true
  // One shared interval — 30s is right for error banners (they persist until fixed)
  setInterval(broadcast, 30_000)
}

export function subscribe(ctrl: Controller): void {
  controllers.add(ctrl)
  ensureInterval()
  // Send current state immediately on connect so the browser doesn't wait 30s
  const notices = queryNotices()
  lastSnapshot = JSON.stringify(notices)
  ctrl.enqueue(encoder.encode(`data: ${JSON.stringify(notices)}\n\n`))
}

export function unsubscribe(ctrl: Controller): void {
  controllers.delete(ctrl)
}

// Force an immediate re-read and broadcast — call after writing to system_notices
// from an API route (e.g. after clearing a notice on successful auth).
export function refresh(): void {
  lastSnapshot = '' // invalidate snapshot so broadcast always pushes
  broadcast()
}
