// meridian — normalises screenpipe activity into structured app sessions
//
// GET /api/worklogs?day=YYYY-MM-DD — the day's drafted/approved/posted worklogs
// for review. Returns the editable Jira comment (the payload `summary`), the
// supporting bullets/next-steps for context, confidence, risk flags, and the
// post status (incl. any last error). Read-only; mutations live in [id]/route.ts.

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { todayString } from '@/lib/date-utils'

export const dynamic = 'force-dynamic'

export interface WorklogBullet {
  kind: string
  text: string
}

export interface WorklogItem {
  id: number
  task_key: string
  window_start: string
  state: string
  confidence: number
  coverage: number
  time_spent_seconds: number
  summary: string
  bullets: WorklogBullet[]
  next_steps: string[]
  risk_flags: string[]
  reasoning: string
  posted_worklog_id: string | null
  last_post_error: string | null
  edited: boolean
}

export interface WorklogsResponse {
  day: string
  items: WorklogItem[]
  counts: Record<string, number>
}

interface RawRow {
  id: number
  task_key: string
  window_start: string
  state: string
  confidence: number
  coverage: number
  time_spent_seconds: number
  payload_json: string
  posted_worklog_id: string | null
  last_post_error: string | null
  edited_at: string | null
}

interface RawBullet { text?: string }
interface RawPayload {
  summary?: string
  what_shipped?: RawBullet[]
  in_progress?: RawBullet[]
  blockers?: RawBullet[]
  decisions?: RawBullet[]
  next_steps?: string[]
  risk_flags?: string[]
  reasoning?: string
}

const BULLET_GROUPS: Array<[keyof RawPayload, string]> = [
  ['what_shipped', 'shipped'],
  ['in_progress', 'in progress'],
  ['blockers', 'blocker'],
  ['decisions', 'decision'],
]

export async function GET(req: Request) {
  const url = new URL(req.url)
  const day = url.searchParams.get('day') || todayString()

  try {
    const db = getDb()
    const rows = db.prepare(`
      SELECT id, task_key, window_start, state, confidence, coverage,
             time_spent_seconds, payload_json, posted_worklog_id,
             last_post_error, edited_at
      FROM pm_worklogs
      WHERE day_utc = ?
      ORDER BY window_start, task_key
    `).all(day) as RawRow[]

    const counts: Record<string, number> = {}
    const items: WorklogItem[] = rows.map(r => {
      counts[r.state] = (counts[r.state] ?? 0) + 1

      let p: RawPayload = {}
      try { p = JSON.parse(r.payload_json) as RawPayload } catch { /* leave empty */ }

      const bullets: WorklogBullet[] = []
      for (const [field, kind] of BULLET_GROUPS) {
        const arr = (p[field] as RawBullet[] | undefined) ?? []
        for (const b of arr) {
          if (b?.text) bullets.push({ kind, text: b.text })
        }
      }

      return {
        id: r.id,
        task_key: r.task_key,
        window_start: r.window_start,
        state: r.state,
        confidence: r.confidence ?? 0,
        coverage: r.coverage ?? 0,
        time_spent_seconds: r.time_spent_seconds ?? 0,
        summary: p.summary ?? '',
        bullets,
        next_steps: p.next_steps ?? [],
        risk_flags: p.risk_flags ?? [],
        reasoning: p.reasoning ?? '',
        posted_worklog_id: r.posted_worklog_id,
        last_post_error: r.last_post_error,
        edited: r.edited_at != null,
      }
    })

    return NextResponse.json({ day, items, counts } satisfies WorklogsResponse)
  } catch (e) {
    console.error('worklogs api error:', e)
    return NextResponse.json({ day, items: [], counts: {} } satisfies WorklogsResponse)
  }
}
