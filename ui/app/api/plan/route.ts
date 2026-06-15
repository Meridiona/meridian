//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GET  /api/plan?date=YYYY-MM-DD  → the day's committed plan + ranked suggestions
//                                    + the full scored board (for the "add" panel).
// POST /api/plan  { action, date?, task_key?, task_keys? }
//        confirm  — replace the day's committed set with task_keys (ordered) + stamp confirmed
//        add      — append one task to the day's plan
//        remove   — drop one task from the day's plan
//        reorder  — set positions from the given ordered task_keys
//        skip     — mark the day skipped (evidence fallback drives Tier-1)
//        reopen   — clear confirmed/skipped so suggestions return (keeps committed rows)
//
// Like the triage flow, the UI records intent in meridian.db; nothing is pushed to
// a tracker here. All writes are idempotent UPSERT/DELETE on the daily_plan tables.

import { NextResponse } from 'next/server'
import { getWriteDb } from '@/lib/db-write'
import { buildPlanResponse, buildAvailable, todayString } from '@/lib/daily-plan'

export const dynamic = 'force-dynamic'

function nowIso(): string {
  return new Date().toISOString().replace(/\.\d+Z$/, 'Z')
}

function validDate(d: unknown): string {
  return typeof d === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(d) ? d : todayString()
}

export async function GET(req: Request) {
  try {
    const url = new URL(req.url)
    const date = validDate(url.searchParams.get('date'))
    return NextResponse.json(buildPlanResponse(date))
  } catch (e) {
    console.error('plan api GET error:', e)
    return NextResponse.json(
      { date: todayString(), has_table: false, confirmed: false, skipped: false, plan: [], suggestions: [], available: [] },
      { status: 200 },
    )
  }
}

interface Body {
  action?: unknown
  date?: unknown
  task_key?: unknown
  task_keys?: unknown
}

export async function POST(req: Request) {
  let body: Body
  try { body = await req.json() } catch { return NextResponse.json({ error: 'bad json' }, { status: 400 }) }

  const action = body.action
  if (typeof action !== 'string') {
    return NextResponse.json({ error: 'action required' }, { status: 400 })
  }
  const date = validDate(body.date)

  try {
    const db = getWriteDb()
    // Schema is owned by Rust migration 041; if the daemon hasn't applied it yet,
    // fail clearly rather than silently dropping the dev's plan.
    const ready = db.prepare(
      "SELECT 1 FROM sqlite_master WHERE type='table' AND name='daily_plan'",
    ).get()
    if (!ready) {
      return NextResponse.json({ error: 'plan storage not ready — restart the meridian daemon' }, { status: 503 })
    }

    const now = nowIso()
    // origin lookup uses the scored board so a committed task keeps a meaningful
    // origin label ("carried over" / "in progress" / …) instead of bare "manual".
    const originFor = (() => {
      let map: Map<string, string> | null = null
      return (key: string): string => {
        if (!map) map = new Map(buildAvailable(db, date).map(a => [a.key, a.origin]))
        return map.get(key) ?? 'manual'
      }
    })()

    const upsertMeta = (confirmedAt: string | null, skipped: number) => {
      db.prepare(`
        INSERT INTO daily_plan_meta (plan_date, confirmed_at, skipped, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(plan_date) DO UPDATE SET
          confirmed_at = excluded.confirmed_at,
          skipped      = excluded.skipped,
          updated_at   = excluded.updated_at
      `).run(date, confirmedAt, skipped, now, now)
    }

    // Replace the day's committed set with `ordered` (idempotent UPSERT + prune).
    const replacePlan = db.transaction((ordered: string[]) => {
      const keep = new Set(ordered)
      const existing = db.prepare('SELECT task_key FROM daily_plan WHERE plan_date = ?').all(date) as Array<{ task_key: string }>
      for (const row of existing) {
        if (!keep.has(row.task_key)) {
          db.prepare('DELETE FROM daily_plan WHERE plan_date = ? AND task_key = ?').run(date, row.task_key)
        }
      }
      ordered.forEach((key, i) => {
        db.prepare(`
          INSERT INTO daily_plan (plan_date, task_key, position, origin, created_at, updated_at)
          VALUES (?, ?, ?, ?, ?, ?)
          ON CONFLICT(plan_date, task_key) DO UPDATE SET position = excluded.position, updated_at = excluded.updated_at
        `).run(date, key, i, originFor(key), now, now)
      })
    })
    const keysFromBody = () => Array.isArray(body.task_keys) ? body.task_keys.filter((k): k is string => typeof k === 'string') : []

    switch (action) {
      case 'confirm': {
        const keys = keysFromBody()
        db.transaction(() => { replacePlan(keys); upsertMeta(now, 0) })()
        break
      }
      case 'set': {
        // Live edit while already confirmed — replace rows, leave meta untouched.
        replacePlan(keysFromBody())
        break
      }
      case 'add': {
        const key = body.task_key
        if (typeof key !== 'string' || !key) return NextResponse.json({ error: 'task_key required' }, { status: 400 })
        const max = db.prepare('SELECT COALESCE(MAX(position), -1) AS m FROM daily_plan WHERE plan_date = ?').get(date) as { m: number }
        db.prepare(`
          INSERT INTO daily_plan (plan_date, task_key, position, origin, created_at, updated_at)
          VALUES (?, ?, ?, ?, ?, ?)
          ON CONFLICT(plan_date, task_key) DO NOTHING
        `).run(date, key, max.m + 1, originFor(key), now, now)
        break
      }
      case 'remove': {
        const key = body.task_key
        if (typeof key !== 'string' || !key) return NextResponse.json({ error: 'task_key required' }, { status: 400 })
        db.prepare('DELETE FROM daily_plan WHERE plan_date = ? AND task_key = ?').run(date, key)
        break
      }
      case 'reorder': {
        const keys = Array.isArray(body.task_keys) ? body.task_keys.filter((k): k is string => typeof k === 'string') : []
        const repos = db.transaction((ordered: string[]) => {
          ordered.forEach((key, i) => {
            db.prepare('UPDATE daily_plan SET position = ?, updated_at = ? WHERE plan_date = ? AND task_key = ?').run(i, now, date, key)
          })
        })
        repos(keys)
        break
      }
      case 'skip':
        upsertMeta(now, 1)
        break
      case 'reopen':
        upsertMeta(null, 0)
        break
      default:
        return NextResponse.json({ error: `unknown action: ${action}` }, { status: 400 })
    }

    // Return the fresh state so the client can reconcile against the server.
    return NextResponse.json(buildPlanResponse(date))
  } catch (e) {
    console.error('plan api POST error:', e)
    return NextResponse.json({ error: 'write failed' }, { status: 500 })
  }
}
