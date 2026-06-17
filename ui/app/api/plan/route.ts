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

const DATE_RE = /^\d{4}-\d{2}-\d{2}$/

// GET: an absent/garbage date harmlessly falls back to today (read-only).
function readDate(d: unknown): string {
  return typeof d === 'string' && DATE_RE.test(d) ? d : todayString()
}

// Writes: an explicitly-supplied date MUST be valid — defaulting a malformed
// date to today would silently mutate the WRONG day's plan. Absent → today.
function writeDate(d: unknown): string | null {
  if (d === undefined || d === null) return todayString()
  return typeof d === 'string' && DATE_RE.test(d) ? d : null
}

export async function GET(req: Request) {
  try {
    const url = new URL(req.url)
    const date = readDate(url.searchParams.get('date'))
    return NextResponse.json(buildPlanResponse(date))
  } catch (e) {
    // Surface backend failure as 500 — a DB/read error must NOT render as a
    // valid empty day (the client can't tell those apart otherwise).
    console.error('plan api GET error:', e)
    return NextResponse.json({ error: 'failed to load plan' }, { status: 500 })
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
  const date = writeDate(body.date)
  if (date === null) {
    return NextResponse.json({ error: 'invalid date (expected YYYY-MM-DD)' }, { status: 400 })
  }

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
    // Score the board ONCE per request and reuse it for both the origin lookup
    // and the response payload below (buildPlanResponse would otherwise re-score).
    const available = buildAvailable(db, date)
    // origin lookup uses the scored board so a committed task keeps a meaningful
    // origin label ("carried over" / "in progress" / …) instead of bare "manual".
    const originMap = new Map(available.map(a => [a.key, a.origin]))
    const originFor = (key: string): string => originMap.get(key) ?? 'manual'

    // Snapshot the ticket's live board fields onto the plan row at write time, so
    // a planned-then-completed task (pruned from pm_tasks once Done) still renders
    // its real title/description/epic on the /plan page instead of a bare key.
    // Returns null when the task isn't on the board (don't clobber an earlier
    // snapshot — see the COALESCE in the UPSERTs below).
    const snapStmt = db.prepare(`
      SELECT title, provider, url, COALESCE(status_raw,'') AS status_raw,
             COALESCE(is_terminal,0) AS is_terminal, due_date,
             description_text, epic_title, parent_key, priority, issue_type, story_points
      FROM pm_tasks WHERE task_key = ?
    `)
    const snapshotFor = (key: string): string | null => {
      const row = snapStmt.get(key)
      return row ? JSON.stringify(row) : null
    }

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
          INSERT INTO daily_plan (plan_date, task_key, position, origin, task_snapshot, created_at, updated_at)
          VALUES (?, ?, ?, ?, ?, ?, ?)
          ON CONFLICT(plan_date, task_key) DO UPDATE SET
            position      = excluded.position,
            updated_at    = excluded.updated_at,
            task_snapshot = COALESCE(excluded.task_snapshot, daily_plan.task_snapshot)
        `).run(date, key, i, originFor(key), snapshotFor(key), now, now)
      })
    })
    const keysFromBody = () => Array.isArray(body.task_keys) ? body.task_keys.filter((k): k is string => typeof k === 'string') : []

    switch (action) {
      case 'confirm': {
        // task_keys MUST be an array — an explicit [] means "clear the plan",
        // but a missing/malformed body must 400, not wipe the day silently.
        if (!Array.isArray(body.task_keys)) {
          return NextResponse.json({ error: 'task_keys array required' }, { status: 400 })
        }
        const keys = keysFromBody()
        db.transaction(() => { replacePlan(keys); upsertMeta(now, 0) })()
        break
      }
      case 'set': {
        // Live edit while already confirmed — replace rows, leave meta untouched.
        if (!Array.isArray(body.task_keys)) {
          return NextResponse.json({ error: 'task_keys array required' }, { status: 400 })
        }
        replacePlan(keysFromBody())
        break
      }
      case 'add': {
        const key = body.task_key
        if (typeof key !== 'string' || !key) return NextResponse.json({ error: 'task_key required' }, { status: 400 })
        const max = db.prepare('SELECT COALESCE(MAX(position), -1) AS m FROM daily_plan WHERE plan_date = ?').get(date) as { m: number }
        db.prepare(`
          INSERT INTO daily_plan (plan_date, task_key, position, origin, task_snapshot, created_at, updated_at)
          VALUES (?, ?, ?, ?, ?, ?, ?)
          ON CONFLICT(plan_date, task_key) DO NOTHING
        `).run(date, key, max.m + 1, originFor(key), snapshotFor(key), now, now)
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
    // Reuse the board scored above (plan writes don't change pm_tasks scoring).
    return NextResponse.json(buildPlanResponse(date, db, available))
  } catch (e) {
    console.error('plan api POST error:', e)
    return NextResponse.json({ error: 'write failed' }, { status: 500 })
  }
}
