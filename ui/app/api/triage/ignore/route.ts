//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// POST /api/triage/ignore  { task_key, code, undo? }
//
// Dismiss an OPTIONAL hygiene defect on a ticket so it stops being surfaced.
// Must-fix defects (due date / description / title) are rejected — they can't be
// ignored. Stored as a JSON array of reason codes in pm_task_curation.ignored_codes.

import { NextResponse } from 'next/server'
import { getWriteDb } from '@/lib/db-write'

export const dynamic = 'force-dynamic'

// Mirror of MUST_FIX in lib/hygiene — these can never be ignored.
const MUST_FIX = new Set(['missing_description', 'thin_description', 'vague_title', 'missing_due_date', 'overdue'])

export async function POST(req: Request) {
  let body: { task_key?: unknown; code?: unknown; undo?: unknown }
  try { body = await req.json() } catch { return NextResponse.json({ error: 'bad json' }, { status: 400 }) }

  const taskKey = body.task_key
  const code = body.code
  if (typeof taskKey !== 'string' || !taskKey) return NextResponse.json({ error: 'task_key required' }, { status: 400 })
  if (typeof code !== 'string' || !code) return NextResponse.json({ error: 'code required' }, { status: 400 })
  if (MUST_FIX.has(code)) return NextResponse.json({ error: 'must-fix issues cannot be ignored' }, { status: 409 })

  try {
    const db = getWriteDb()
    const row = db.prepare('SELECT ignored_codes FROM pm_task_curation WHERE task_key = ?').get(taskKey) as { ignored_codes: string } | undefined
    if (!row) return NextResponse.json({ error: 'unknown task_key' }, { status: 404 })

    let codes: string[] = []
    try { codes = JSON.parse(row.ignored_codes) } catch { codes = [] }
    const set = new Set(codes)
    if (body.undo === true) set.delete(code)
    else set.add(code)

    db.prepare('UPDATE pm_task_curation SET ignored_codes = ? WHERE task_key = ?')
      .run(JSON.stringify([...set]), taskKey)
    return NextResponse.json({ ok: true, task_key: taskKey, ignored: [...set] })
  } catch (e) {
    console.error('triage ignore error:', e)
    return NextResponse.json({ error: 'write failed' }, { status: 500 })
  }
}
