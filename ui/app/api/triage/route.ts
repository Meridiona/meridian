//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GET /api/triage — the onboarding board-cleanup working set. Reads the machine
// verdicts the daemon wrote into pm_task_curation (after each PM sync) joined with
// the ticket, worst-first (needs_detail → looks_stale → not_sure → ready), and
// turns each reason code into a friendly hint. Read-only; the decision write lives
// in decision/route.ts.

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'

export const dynamic = 'force-dynamic'

export type TriageBucket = 'ready' | 'needs_detail' | 'looks_stale' | 'not_sure'

export interface TriageReason {
  code: string
  hint: string
}

export interface TriageTicket {
  task_key: string
  provider: string
  title: string
  url: string
  description_excerpt: string
  bucket: TriageBucket
  reasons: TriageReason[]
  decision: string | null
  snoozed_until: string | null
}

export interface TriageResponse {
  items: TriageTicket[]
  counts: {
    total: number
    ready: number
    needs_detail: number
    looks_stale: number
    not_sure: number
    needs_attention: number
    undecided: number
  }
  has_run: boolean
}

// Mirror of TriageReason::hint() in the Rust engine — kept in sync by hand.
function reasonHint(code: string, detail: Record<string, number> | undefined): string {
  switch (code) {
    case 'in_progress': return 'Marked in progress on the board.'
    case 'due_soon': return (detail?.in_days ?? 1) <= 0 ? 'Due today.' : `Due in ${detail?.in_days} day(s).`
    case 'in_sprint': return 'In the active sprint.'
    case 'start_date_reached': return 'Its start date has passed.'
    case 'missing_description': return 'No description — nothing to match your work against.'
    case 'thin_description': return `Description is only ${detail?.chars} characters — add a little detail.`
    case 'vague_title': return 'Title is generic — make it specific.'
    case 'no_context_anchor': return 'No epic or parent to anchor it.'
    case 'missing_due_date': return "No due date — add one so Meridian knows when it's live."
    case 'no_activity_since': return `No board activity in ${detail?.days} days.`
    case 'not_started': return 'Still sitting in a not-started column.'
    case 'no_due_date': return 'No due date set.'
    case 'overdue_long': return `Overdue by ${detail?.by_days} days with no movement.`
    case 'far_future_due': return `Not due for ${detail?.in_days} days — planned, not current work.`
    case 'not_in_sprint': return 'Not in any sprint.'
    case 'already_done': return 'Already marked done.'
    case 'no_activity_signal': return "Open, but nothing yet says it's active."
    case 'unreadable_updated_at': return "Couldn't read its last-updated time."
    default: return code
  }
}

interface RawReason { code: string; detail?: Record<string, number> }

interface RawRow {
  task_key: string
  provider: string
  title: string
  url: string
  description_text: string
  bucket: TriageBucket
  reasons_json: string
  decision: string | null
  snoozed_until: string | null
}

export async function GET() {
  try {
    const db = getDb()
    // Table only exists after migration 038 + a sync; tolerate a fresh DB.
    const exists = db.prepare(
      "SELECT 1 FROM sqlite_master WHERE type='table' AND name='pm_task_curation'",
    ).get()
    if (!exists) {
      return NextResponse.json({ items: [], counts: emptyCounts(), has_run: false } as TriageResponse)
    }

    const rows = db.prepare(`
      SELECT t.task_key, t.provider, t.title, t.url,
             COALESCE(t.description_text,'') AS description_text,
             c.bucket, c.reasons_json, c.decision, c.snoozed_until
      FROM pm_task_curation c
      JOIN pm_tasks t ON t.task_key = c.task_key
      ORDER BY CASE c.bucket
        WHEN 'needs_detail' THEN 0
        WHEN 'looks_stale'  THEN 1
        WHEN 'not_sure'     THEN 2
        ELSE 3 END, t.task_key
    `).all() as RawRow[]

    const items: TriageTicket[] = rows.map(r => {
      let reasons: TriageReason[] = []
      try {
        const parsed = JSON.parse(r.reasons_json) as RawReason[]
        reasons = parsed.map(p => ({ code: p.code, hint: reasonHint(p.code, p.detail) }))
      } catch { /* leave empty */ }
      const desc = r.description_text.trim()
      return {
        task_key: r.task_key,
        provider: r.provider || 'jira',
        title: r.title,
        url: r.url || '',
        description_excerpt: desc.length > 160 ? desc.slice(0, 157) + '…' : desc,
        bucket: r.bucket,
        reasons,
        decision: r.decision,
        snoozed_until: r.snoozed_until,
      }
    })

    const counts = emptyCounts()
    counts.total = items.length
    for (const it of items) {
      counts[it.bucket] += 1
      if (it.bucket !== 'ready') {
        counts.needs_attention += 1
        if (!it.decision) counts.undecided += 1
      }
    }

    return NextResponse.json({ items, counts, has_run: true } as TriageResponse)
  } catch (e) {
    console.error('triage api error:', e)
    return NextResponse.json({ items: [], counts: emptyCounts(), has_run: false } as TriageResponse)
  }
}

function emptyCounts(): TriageResponse['counts'] {
  return { total: 0, ready: 0, needs_detail: 0, looks_stale: 0, not_sure: 0, needs_attention: 0, undecided: 0 }
}
