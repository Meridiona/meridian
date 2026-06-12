//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// POST /api/triage/decision  { task_key, decision: keep|excluded|snoozed, snooze_days? }
//
// Records the user's board-cleanup decision into pm_task_curation. Like the
// worklog flow, the UI never mutates the PM tracker itself — it records intent in
// the DB. An `excluded` decision immediately stops the ticket being a
// classification candidate (the Python tagger's candidate query honours it); the
// daemon's apply-sweep is what later pushes a close back to the tracker.

import { NextResponse } from 'next/server'
import { getWriteDb } from '@/lib/db-write'

export const dynamic = 'force-dynamic'

const DECISIONS = new Set(['keep', 'excluded', 'snoozed'])

function nowIso(): string {
  return new Date().toISOString().replace(/\.\d+Z$/, 'Z')
}

function snoozeUntil(days: number): string {
  const d = new Date()
  d.setDate(d.getDate() + days)
  return d.toISOString().replace(/\.\d+Z$/, 'Z')
}

export async function POST(req: Request) {
  let body: { task_key?: unknown; decision?: unknown; snooze_days?: unknown }
  try { body = await req.json() } catch { return NextResponse.json({ error: 'bad json' }, { status: 400 }) }

  const taskKey = body.task_key
  const decision = body.decision
  if (typeof taskKey !== 'string' || !taskKey) {
    return NextResponse.json({ error: 'task_key required' }, { status: 400 })
  }
  if (typeof decision !== 'string' || !DECISIONS.has(decision)) {
    return NextResponse.json({ error: 'decision must be keep|excluded|snoozed' }, { status: 400 })
  }
  const snoozeDays = typeof body.snooze_days === 'number' ? Math.max(1, Math.floor(body.snooze_days)) : 7
  const snoozedUntil = decision === 'snoozed' ? snoozeUntil(snoozeDays) : null

  try {
    const db = getWriteDb()
    const res = db.prepare(`
      UPDATE pm_task_curation
      SET decision = ?, decided_at = ?, snoozed_until = ?
      WHERE task_key = ?
    `).run(decision, nowIso(), snoozedUntil, taskKey)

    if (res.changes === 0) {
      return NextResponse.json({ error: 'unknown task_key' }, { status: 404 })
    }
    return NextResponse.json({ ok: true, task_key: taskKey, decision, snoozed_until: snoozedUntil })
  } catch (e) {
    console.error('triage decision error:', e)
    return NextResponse.json({ error: 'write failed' }, { status: 500 })
  }
}
