// meridian — normalises screenpipe activity into structured app sessions
//
// Worklog mutations — the human-in-the-loop write path. The UI never posts to
// Jira itself; it only records intent in the DB and the daemon's approved-sweep
// (~60s) posts. A POSTED worklog is immutable here.
//
//   PATCH /api/worklogs/:id   { summary }                  → edit the comment
//   POST  /api/worklogs/:id    { action: approve|reject|unapprove }
//
//     approve   drafted/skipped → approved   (daemon will post it)
//     reject    drafted/approved → skipped   (dismiss; won't post)
//     unapprove approved → drafted           (pull back before the sweep)

import { NextResponse } from 'next/server'
import { getWriteDb } from '@/lib/db-write'

export const dynamic = 'force-dynamic'

type Ctx = { params: Promise<{ id: string }> }

function nowIso(): string {
  return new Date().toISOString().replace(/\.\d+Z$/, 'Z')
}

function parseId(raw: string): number | null {
  const id = Number(raw)
  return Number.isInteger(id) && id > 0 ? id : null
}

interface StateRow { state: string; payload_json: string }

// ── Edit the Jira comment (the payload `summary`) ──────────────────────────
export async function PATCH(req: Request, ctx: Ctx) {
  const id = parseId((await ctx.params).id)
  if (id == null) return NextResponse.json({ error: 'bad id' }, { status: 400 })

  let body: { summary?: unknown }
  try { body = await req.json() } catch { return NextResponse.json({ error: 'bad json' }, { status: 400 }) }
  if (typeof body.summary !== 'string') {
    return NextResponse.json({ error: 'summary must be a string' }, { status: 400 })
  }
  const summary = body.summary

  try {
    const db = getWriteDb()
    const row = db.prepare('SELECT state, payload_json FROM pm_worklogs WHERE id = ?').get(id) as StateRow | undefined
    if (!row) return NextResponse.json({ error: 'not found' }, { status: 404 })
    if (row.state === 'posted') {
      return NextResponse.json({ error: 'worklog already posted to Jira — cannot edit' }, { status: 409 })
    }

    let original = ''
    try { original = (JSON.parse(row.payload_json)?.summary as string) ?? '' } catch { /* ignore */ }

    // Editing an approved row pulls it back to drafted — content changed, so it
    // must be re-approved before it posts.
    const nextState = row.state === 'approved' ? 'drafted' : row.state

    const tx = db.transaction(() => {
      db.prepare(`
        UPDATE pm_worklogs
        SET payload_json = json_set(payload_json, '$.summary', ?),
            state = ?, edited_at = ?, last_post_error = NULL
        WHERE id = ?
      `).run(summary, nextState, nowIso(), id)

      db.prepare(`
        INSERT INTO pm_worklog_feedback (pm_worklog_id, feedback_kind, original_text, edited_text)
        VALUES (?, 'edit', ?, ?)
      `).run(id, original, summary)
    })
    tx()

    return NextResponse.json({ ok: true, id, state: nextState })
  } catch (e) {
    console.error('worklog edit error:', e)
    return NextResponse.json({ error: 'edit failed' }, { status: 500 })
  }
}

// ── State transitions (approve / reject / unapprove) ───────────────────────
export async function POST(req: Request, ctx: Ctx) {
  const id = parseId((await ctx.params).id)
  if (id == null) return NextResponse.json({ error: 'bad id' }, { status: 400 })

  let body: { action?: unknown; correctedTaskKey?: unknown; correctedToUntracked?: unknown }
  try { body = await req.json() } catch { return NextResponse.json({ error: 'bad json' }, { status: 400 }) }
  const action = body.action
  if (action !== 'approve' && action !== 'reject' && action !== 'unapprove') {
    return NextResponse.json({ error: 'action must be approve|reject|unapprove' }, { status: 400 })
  }

  // Attribution label — reject only. The reviewer optionally says where the
  // time should have gone (a different ticket, or untracked); that is the
  // ground-truth correction for the classifier. Ignored for other actions.
  const correctedTaskKey =
    action === 'reject' && typeof body.correctedTaskKey === 'string' && body.correctedTaskKey.trim()
      ? body.correctedTaskKey.trim()
      : null
  const correctedToUntracked = action === 'reject' && body.correctedToUntracked === true ? 1 : 0

  try {
    const db = getWriteDb()
    const row = db.prepare('SELECT state FROM pm_worklogs WHERE id = ?').get(id) as { state: string } | undefined
    if (!row) return NextResponse.json({ error: 'not found' }, { status: 404 })
    if (row.state === 'posted') {
      return NextResponse.json({ error: 'worklog already posted to Jira' }, { status: 409 })
    }

    // The review action is itself an eval signal (approve = weak positive,
    // reject = the worklog should not exist). The state column alone is lossy —
    // it only holds the *latest* state — so we also append an immutable
    // pm_worklog_feedback row per action, mirroring how edits are recorded.
    // `note` carries the state transition for later failure-signature derivation.
    const transition = (next: string) => `${row.state}→${next}`

    const tx = db.transaction((next: string) => {
      if (action === 'approve') {
        db.prepare(`
          UPDATE pm_worklogs SET state = 'approved', approved_at = ?, last_post_error = NULL WHERE id = ?
        `).run(nowIso(), id)
      } else if (action === 'reject') {
        db.prepare(`UPDATE pm_worklogs SET state = 'skipped' WHERE id = ?`).run(id)
      } else {
        db.prepare(`UPDATE pm_worklogs SET state = 'drafted', approved_at = NULL WHERE id = ?`).run(id)
      }
      db.prepare(`
        INSERT INTO pm_worklog_feedback (pm_worklog_id, feedback_kind, note, corrected_task_key, corrected_to_untracked)
        VALUES (?, ?, ?, ?, ?)
      `).run(id, action, transition(next), correctedTaskKey, correctedToUntracked)
    })

    let next: string
    if (action === 'approve') next = 'approved'
    else if (action === 'reject') next = 'skipped'
    else next = 'drafted'
    tx(next)

    return NextResponse.json({ ok: true, id, state: next })
  } catch (e) {
    console.error('worklog action error:', e)
    return NextResponse.json({ error: 'action failed' }, { status: 500 })
  }
}
