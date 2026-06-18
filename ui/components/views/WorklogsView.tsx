//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useCallback, useEffect, useState } from 'react'
import { fmtDur, fmtClock, TaskKey, ConfidenceRing } from '@/components/atoms'
import type { WorklogItem, WorklogsResponse } from '@/app/api/worklogs/route'
import { load as loadData, mutate } from '@/lib/bridge'

// Local YYYY-MM-DD for `d` days from today (negative = past).
function dayString(offsetDays = 0): string {
  const d = new Date()
  d.setDate(d.getDate() + offsetDays)
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}

// Human label for a worklog's tracker (provider snapshot on the row).
function providerLabel(provider: string): string {
  switch (provider) {
    case 'jira': return 'Jira'
    case 'linear': return 'Linear'
    case 'github': return 'GitHub'
    default: return provider || 'Jira'
  }
}

// Where the reviewer says the time should have gone, supplied on reject.
// Empty = plain dismissal. correctedToUntracked wins if both are set server-side.
type RejectCorrection = { correctedTaskKey?: string; correctedToUntracked?: boolean }

const STATE_STYLE: Record<string, { label: string; color: string }> = {
  drafted:  { label: 'Draft',    color: 'var(--ink-3)' },
  approved: { label: 'Approved', color: 'var(--accent)' },
  posted:   { label: 'Posted',   color: '#2F9E44' },
  skipped:  { label: 'Dismissed', color: 'var(--ink-4)' },
  failed:   { label: 'Failed',   color: '#E03131' },
}

export default function WorklogsView() {
  const [day, setDay] = useState<string>(dayString(0))
  const [items, setItems] = useState<WorklogItem[]>([])
  const [counts, setCounts] = useState<Record<string, number>>({})
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<number | null>(null)

  const load = useCallback((d: string) => {
    // get_worklogs (Rust) in the Tauri window, /api/worklogs in a browser.
    // Mutations (/api/worklogs/[id], below) stay on fetch until those write
    // routes are ported.
    loadData<WorklogsResponse>(`/api/worklogs?day=${d}`, 'get_worklogs', { day: d })
      .then((res) => { setItems(res.items ?? []); setCounts(res.counts ?? {}); setLoading(false) })
      .catch(() => setLoading(false))
  }, [])

  useEffect(() => {
    setLoading(true)
    load(day)
    // Poll so approved → posted transitions (driven by the daemon sweep) show up.
    const id = setInterval(() => load(day), 30_000)
    return () => clearInterval(id)
  }, [day, load])

  // Run a worklog write (dual-path via the bridge `mutate`), with busy state +
  // reload-on-settle. The bridge throws the server's error text on failure;
  // surface it. Each call bakes `id` into both the URL (browser) and the body
  // (command). PATCH/POST/DELETE pick the browser verb.
  async function run(id: number, fn: () => Promise<unknown>) {
    setBusy(id)
    try {
      await fn()
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Action failed')
    } finally {
      setBusy(null)
      load(day)
    }
  }

  const act = (id: number, action: 'approve' | 'unapprove') =>
    run(id, () => mutate(`/api/worklogs/${id}`, 'worklog_action', { id, action }))

  // Reject carries an optional attribution correction: where the time should
  // have gone. Empty object = a plain dismissal with no target supplied.
  const reject = (id: number, correction: RejectCorrection) =>
    run(id, () => mutate(`/api/worklogs/${id}`, 'worklog_action', { id, action: 'reject', ...correction }))

  const saveEdit = (id: number, summary: string) =>
    run(id, () => mutate(`/api/worklogs/${id}`, 'edit_worklog', { id, summary }, 'PATCH'))

  const draftedIds = items.filter(i => i.state === 'drafted' && i.summary.trim()).map(i => i.id)
  async function approveAll() {
    setBusy(-1)
    try {
      await Promise.all(draftedIds.map(id =>
        mutate(`/api/worklogs/${id}`, 'worklog_action', { id, action: 'approve' })))
    } finally { setBusy(null); load(day) }
  }

  const isToday = day === dayString(0)

  return (
    <div className="space-y-8">
      <header className="rise flex items-end justify-between gap-4">
        <div>
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Worklog review</p>
          <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>
            Approve before it posts
          </h1>
          <p className="mt-3 text-[14px] max-w-prose" style={{ color: 'var(--ink-2)' }}>
            Nothing reaches your tracker until you approve it. Edit the comment if it&apos;s off, then approve —
            the daemon posts approved worklogs within a minute.
          </p>
        </div>
        <div className="text-right shrink-0">
          <div className="flex items-center gap-1 justify-end">
            <button onClick={() => setDay(d => shiftDay(d, -1))}
              className="px-2 py-1 rounded-md text-[13px]" style={{ color: 'var(--ink-3)', border: '1px solid var(--rule-2)' }}>←</button>
            <span className="font-mono tnum text-[12px] px-2" style={{ color: 'var(--ink-2)' }}>{isToday ? 'Today' : day}</span>
            <button onClick={() => setDay(d => shiftDay(d, 1))} disabled={isToday}
              className="px-2 py-1 rounded-md text-[13px]" style={{ color: isToday ? 'var(--ink-4)' : 'var(--ink-3)', border: '1px solid var(--rule-2)' }}>→</button>
          </div>
          <p className="text-[11px] mt-2" style={{ color: 'var(--ink-3)' }}>
            {(counts.drafted ?? 0)} draft · {(counts.approved ?? 0)} approved · {(counts.posted ?? 0)} posted
          </p>
        </div>
      </header>

      {draftedIds.length > 0 && (
        <div className="flex items-center gap-3">
          <button onClick={approveAll} disabled={busy === -1}
            className="px-3 py-1.5 rounded-md text-[12px] transition-colors"
            style={{ background: 'var(--accent)', color: 'var(--paper)' }}>
            {busy === -1 ? 'Approving…' : `Approve all ${draftedIds.length} drafts`}
          </button>
          <span className="text-[11px]" style={{ color: 'var(--ink-4)' }}>posts everything you haven&apos;t edited away</span>
        </div>
      )}

      {loading ? (
        <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
      ) : items.length === 0 ? (
        <div className="py-16 text-center rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
          <p className="type-empty" style={{ color: 'var(--ink-2)' }}>No worklogs {isToday ? 'yet today' : 'this day'}.</p>
          <p className="text-[12px] mt-2" style={{ color: 'var(--ink-3)' }}>
            They appear here as the daemon drafts them, hour by hour.
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          {items.map(w => (
            <WorklogCard key={w.id} w={w} busy={busy === w.id}
              onApprove={() => act(w.id, 'approve')}
              onReject={(correction) => reject(w.id, correction)}
              onUnapprove={() => act(w.id, 'unapprove')}
              onSave={(s) => saveEdit(w.id, s)} />
          ))}
        </div>
      )}
    </div>
  )
}

function shiftDay(d: string, by: number): string {
  const dt = new Date(`${d}T12:00:00`)
  dt.setDate(dt.getDate() + by)
  const today = new Date(); today.setHours(12, 0, 0, 0)
  if (dt > today) return d // never go past today
  const y = dt.getFullYear(); const m = String(dt.getMonth() + 1).padStart(2, '0'); const day = String(dt.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}

type Candidate = { key: string; title: string }

function WorklogCard({ w, busy, onApprove, onReject, onUnapprove, onSave }: {
  w: WorklogItem
  busy: boolean
  onApprove: () => void
  onReject: (correction: RejectCorrection) => void
  onUnapprove: () => void
  onSave: (summary: string) => void
}) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(w.summary)
  const [showEvidence, setShowEvidence] = useState(false)
  // Reject flow: open the picker, choose a target (a task_key, '__untracked__',
  // or '__unknown__' = dismiss without a target), then confirm.
  const [rejecting, setRejecting] = useState(false)
  const [candidates, setCandidates] = useState<Candidate[] | null>(null)
  const [target, setTarget] = useState<string>('__unknown__')
  const st = STATE_STYLE[w.state] ?? { label: w.state, color: 'var(--ink-3)' }
  const posted = w.state === 'posted'

  useEffect(() => { setDraft(w.summary) }, [w.summary])

  async function openReject() {
    setRejecting(true)
    setTarget('__unknown__')
    if (candidates == null) {
      try {
        // get_tasks (Rust) in the Tauri window, /api/tasks in a browser.
        const data = await loadData<{ tasks: { key: string; title: string }[] }>('/api/tasks', 'get_tasks')
        // Don't offer the worklog's own ticket as the "should have gone" target.
        setCandidates((data.tasks ?? [])
          .map((t) => ({ key: t.key, title: t.title }))
          .filter((c: Candidate) => c.key !== w.task_key))
      } catch { setCandidates([]) }
    }
  }

  function confirmReject() {
    const correction: RejectCorrection =
      target === '__untracked__' ? { correctedToUntracked: true }
        : target === '__unknown__' ? {}
          : { correctedTaskKey: target }
    onReject(correction)
    setRejecting(false)
  }

  return (
    <div className="rounded-xl border overflow-hidden" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
      <div className="px-5 py-4">
        {/* meta row */}
        <div className="flex items-center gap-3 min-w-0">
          {w.task_url ? (
            <a href={w.task_url} target="_blank" rel="noopener noreferrer" title={`Open ${w.task_key} in ${providerLabel(w.provider)}`}
              className="flex items-center gap-2 min-w-0 group">
              <TaskKey keyId={w.task_key} />
              {w.task_title && (
                <span className="text-[12px] truncate group-hover:underline" style={{ color: 'var(--ink-2)' }}>
                  {w.task_title}
                </span>
              )}
              <span className="text-[10px] shrink-0" style={{ color: 'var(--ink-4)' }}>↗</span>
            </a>
          ) : (
            <span className="flex items-center gap-2 min-w-0">
              <TaskKey keyId={w.task_key} />
              {w.task_title && (
                <span className="text-[12px] truncate" style={{ color: 'var(--ink-2)' }}>{w.task_title}</span>
              )}
            </span>
          )}
          <span className="text-[10px] uppercase tracking-[0.12em] shrink-0" style={{ color: 'var(--ink-4)' }}>{providerLabel(w.provider)}</span>
          <span className="font-mono tnum text-[11px]" style={{ color: 'var(--ink-3)' }}>{fmtClock(w.window_start)}</span>
          <span className="text-[11px]" style={{ color: 'var(--ink-4)' }}>·</span>
          <span className="font-mono tnum text-[11px]" style={{ color: 'var(--ink-3)' }}>{fmtDur(w.time_spent_seconds)}</span>
          <ConfidenceRing value={w.confidence} />
          {w.edited && <span className="text-[10px] uppercase tracking-[0.12em]" style={{ color: 'var(--ink-4)' }}>edited</span>}
          <span className="ml-auto text-[10px] uppercase tracking-[0.14em] px-2 py-0.5 rounded"
            style={{ color: st.color, border: `1px solid ${st.color}` }}>{st.label}</span>
        </div>

        {/* risk flags */}
        {w.risk_flags.length > 0 && (
          <div className="flex flex-wrap gap-1.5 mt-2">
            {w.risk_flags.map(f => (
              <span key={f} className="text-[10px] px-1.5 py-0.5 rounded font-mono"
                style={{ background: 'var(--tint)', color: '#B45309', border: '1px solid var(--rule-2)' }}>⚑ {f}</span>
            ))}
          </div>
        )}

        {/* comment — editable */}
        <div className="mt-3">
          {editing && !posted ? (
            <div>
              <textarea value={draft} onChange={e => setDraft(e.target.value)} rows={4}
                className="w-full px-3 py-2 rounded-md text-[13px] leading-relaxed"
                style={{ background: 'var(--surface-2)', border: '1px solid var(--rule-2)', color: 'var(--ink)', outline: 'none', resize: 'vertical' }} />
              <div className="flex items-center gap-2 mt-2">
                <button onClick={() => { onSave(draft); setEditing(false) }} disabled={busy}
                  className="px-3 py-1 rounded-md text-[12px]" style={{ background: 'var(--ink)', color: 'var(--paper)' }}>Save</button>
                <button onClick={() => { setDraft(w.summary); setEditing(false) }}
                  className="px-3 py-1 rounded-md text-[12px]" style={{ color: 'var(--ink-3)', border: '1px solid var(--rule-2)' }}>Cancel</button>
                <span className="text-[10px]" style={{ color: 'var(--ink-4)' }}>saving re-drafts an approved worklog</span>
              </div>
            </div>
          ) : (
            <p className="text-[13px] leading-relaxed whitespace-pre-wrap" style={{ color: w.summary ? 'var(--ink)' : 'var(--ink-4)' }}>
              {w.summary || '(empty — nothing to post; edit to add a comment)'}
            </p>
          )}
        </div>

        {/* post error */}
        {w.last_post_error && (
          <p className="text-[11px] mt-2 font-mono" style={{ color: '#E03131' }}>post error: {w.last_post_error}</p>
        )}
        {posted && w.posted_worklog_id && (
          <p className="text-[11px] mt-2" style={{ color: '#2F9E44' }}>✓ posted to {providerLabel(w.provider)} · {w.posted_worklog_id}</p>
        )}

        {/* evidence toggle */}
        {(w.bullets.length > 0 || w.next_steps.length > 0) && (
          <button onClick={() => setShowEvidence(v => !v)} className="text-[11px] mt-3" style={{ color: 'var(--ink-3)' }}>
            {showEvidence ? '− hide' : '+ show'} supporting detail
          </button>
        )}
        {showEvidence && (
          <div className="mt-2 pl-3 border-l space-y-1" style={{ borderColor: 'var(--rule-2)' }}>
            {w.bullets.map((b, i) => (
              <p key={i} className="text-[12px]" style={{ color: 'var(--ink-2)' }}>
                <span className="text-[10px] uppercase tracking-[0.1em] mr-1.5" style={{ color: 'var(--ink-4)' }}>{b.kind}</span>
                {b.text}
              </p>
            ))}
            {w.next_steps.length > 0 && (
              <p className="text-[12px] pt-1" style={{ color: 'var(--ink-3)' }}>
                <span className="text-[10px] uppercase tracking-[0.1em] mr-1.5" style={{ color: 'var(--ink-4)' }}>next</span>
                {w.next_steps.join(' · ')}
              </p>
            )}
          </div>
        )}

        {/* actions */}
        {!posted && (
          <div className="flex items-center gap-2 mt-4">
            {w.state !== 'approved' ? (
              <button onClick={onApprove} disabled={busy || !w.summary.trim()}
                className="px-3 py-1.5 rounded-md text-[12px] transition-colors"
                style={{ background: w.summary.trim() ? 'var(--accent)' : 'var(--rule-2)', color: 'var(--paper)' }}>
                Approve → post
              </button>
            ) : (
              <button onClick={onUnapprove} disabled={busy}
                className="px-3 py-1.5 rounded-md text-[12px]"
                style={{ color: 'var(--ink-2)', border: '1px solid var(--rule-2)' }}>
                Hold (un-approve)
              </button>
            )}
            {!editing && (
              <button onClick={() => setEditing(true)} disabled={busy}
                className="px-3 py-1.5 rounded-md text-[12px]" style={{ color: 'var(--ink-2)', border: '1px solid var(--rule-2)' }}>
                Edit
              </button>
            )}
            {w.state !== 'skipped' && (
              <button onClick={openReject} disabled={busy || rejecting}
                className="px-3 py-1.5 rounded-md text-[12px] ml-auto" style={{ color: 'var(--ink-3)' }}>
                Dismiss
              </button>
            )}
          </div>
        )}

        {/* reject → attribution picker: where should this time have gone? */}
        {rejecting && !posted && (
          <div className="mt-3 rounded-md p-3" style={{ background: 'var(--surface-2)', border: '1px solid var(--rule-2)' }}>
            <p className="text-[12px] mb-2" style={{ color: 'var(--ink-2)' }}>
              Where should this time have gone? <span style={{ color: 'var(--ink-4)' }}>(helps Meridian learn)</span>
            </p>
            <div className="space-y-1 max-h-48 overflow-y-auto">
              {candidates == null ? (
                <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>Loading tickets…</p>
              ) : (
                <>
                  {candidates.map(c => (
                    <label key={c.key} className="flex items-center gap-2 text-[12px] cursor-pointer py-0.5" style={{ color: 'var(--ink)' }}>
                      <input type="radio" name={`reject-${w.id}`} checked={target === c.key} onChange={() => setTarget(c.key)} />
                      <span className="font-mono">{c.key}</span>
                      <span className="truncate" style={{ color: 'var(--ink-2)' }}>{c.title}</span>
                    </label>
                  ))}
                  <label className="flex items-center gap-2 text-[12px] cursor-pointer py-0.5" style={{ color: 'var(--ink)' }}>
                    <input type="radio" name={`reject-${w.id}`} checked={target === '__untracked__'} onChange={() => setTarget('__untracked__')} />
                    Untracked / personal
                  </label>
                  <label className="flex items-center gap-2 text-[12px] cursor-pointer py-0.5" style={{ color: 'var(--ink-3)' }}>
                    <input type="radio" name={`reject-${w.id}`} checked={target === '__unknown__'} onChange={() => setTarget('__unknown__')} />
                    Just dismiss — not sure
                  </label>
                </>
              )}
            </div>
            <div className="flex items-center gap-2 mt-3">
              <button onClick={confirmReject} disabled={busy}
                className="px-3 py-1 rounded-md text-[12px]" style={{ background: 'var(--ink)', color: 'var(--paper)' }}>
                Dismiss worklog
              </button>
              <button onClick={() => setRejecting(false)} disabled={busy}
                className="px-3 py-1 rounded-md text-[12px]" style={{ color: 'var(--ink-3)', border: '1px solid var(--rule-2)' }}>
                Cancel
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
