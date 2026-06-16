//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// SSE broadcast singleton for the in-app notification banner channel — the
// banner-side mirror of notices-store. One shared 30s poll of the notifications
// table, one Set of open controllers, regardless of how many tabs are open.
// State lives on globalThis to survive Next.js HMR re-evaluation.

import getDb from '@/lib/db'
import { activeBanners } from '@/lib/notifications'

export interface BannerNotification {
  id: number
  event_key: string
  severity: 'info' | 'warning' | 'error'
  title: string
  body: string
  deep_link: string | null
  created_at: string
}

type Controller = ReadableStreamDefaultController<Uint8Array>

const g = globalThis as typeof globalThis & {
  _meridianNotifControllers?: Set<Controller>
  _meridianNotifInterval?: ReturnType<typeof setInterval>
  _meridianNotifSnapshot?: string
}
if (!g._meridianNotifControllers) g._meridianNotifControllers = new Set()

const encoder = new TextEncoder()

function controllers(): Set<Controller> { return g._meridianNotifControllers! }

function query(): BannerNotification[] {
  try {
    return activeBanners(getDb()).map(r => ({
      id: r.id, event_key: r.event_key, severity: r.severity,
      title: r.title, body: r.body, deep_link: r.deep_link, created_at: r.created_at,
    }))
  } catch {
    return []
  }
}

function broadcast() {
  const ctrls = controllers()
  if (ctrls.size === 0) return
  const snapshot = JSON.stringify(query())
  if (snapshot === g._meridianNotifSnapshot) return
  g._meridianNotifSnapshot = snapshot
  const payload = encoder.encode(`data: ${snapshot}\n\n`)
  for (const ctrl of ctrls) {
    try { ctrl.enqueue(payload) } catch { ctrls.delete(ctrl) }
  }
}

function ensureInterval() {
  if (g._meridianNotifInterval != null) return
  g._meridianNotifInterval = setInterval(broadcast, 30_000)
}

export function subscribe(ctrl: Controller): void {
  controllers().add(ctrl)
  ensureInterval()
  const snapshot = JSON.stringify(query())
  g._meridianNotifSnapshot = snapshot
  ctrl.enqueue(encoder.encode(`data: ${snapshot}\n\n`))
}

export function unsubscribe(ctrl: Controller): void {
  controllers().delete(ctrl)
  // Stop the shared poll once nobody is listening — otherwise the 30s interval
  // lives on globalThis forever (broadcast() early-returns, so it's a no-op
  // timer, but a needless one). subscribe() re-arms it via ensureInterval().
  if (controllers().size === 0 && g._meridianNotifInterval != null) {
    clearInterval(g._meridianNotifInterval)
    g._meridianNotifInterval = undefined
  }
}

/** Force an immediate re-read + broadcast — call after a dismiss write. */
export function refresh(): void {
  g._meridianNotifSnapshot = '' // invalidate so broadcast always pushes
  broadcast()
}
