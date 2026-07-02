//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// One ticket in the Board Cleanup swipe-card queue — ONE card, no second
// popup. Every hygiene issue's fix control (date picker, text, priority,
// etc.) renders inline right on the card, so fixing something is a single
// click (Fix, no separate dialog to open first). Prev/next only NAVIGATE the
// queue (no drag-commit physics like ReviewCard — Cleanup decisions always go
// through an explicit button, never a swipe distance). Must-fix cards get no
// Keep escape AND no forward-nav escape (Next/ArrowRight both no-op — see
// CleanupOverlay's `canAdvance`) until every must-fix issue is resolved —
// matching the old Cleanup page's "can't be ignored" rule for missing due
// date/description/title, and preventing paging straight past a must-fix
// card to a trailing reviewable one just to reach "board is healthy".

'use client'

import { useEffect, useState } from 'react'
import { ProviderGlyph } from '@/components/atoms'
import type { TaskSummary } from '@/lib/api-types'
import type { HygieneIssue } from '@/lib/hygiene'
import { load, mutate } from '@/lib/bridge'

export type CleanupGroup = 'must' | 'nice' | 'review'

const GROUP_META: Record<CleanupGroup, { label: string; color: string }> = {
  must: { label: 'Must fix', color: 'var(--severity-must)' },
  nice: { label: 'Nice to fix', color: 'var(--severity-nice)' },
  review: { label: 'Review', color: 'var(--t-faint)' },
}

const PRIORITIES = ['Highest', 'High', 'Medium', 'Low', 'Lowest']

export function CleanupCard({
  task, issues, group, onIgnore, onApplied, onKeep, onPrev, onNext, hasPrev, hasNext,
}: {
  task: TaskSummary
  issues: HygieneIssue[]
  group: CleanupGroup
  onIgnore: (code: string) => void
  onApplied: (code: string) => void
  onKeep: () => void
  onPrev: () => void
  onNext: () => void
  hasPrev: boolean
  hasNext: boolean
}) {
  const meta = GROUP_META[group]

  return (
    <div className="flex flex-col items-center gap-5">
      <div className="relative w-full rounded-2xl overflow-hidden bg-card"
        style={{
          border: '1px solid var(--t-card-border)',
          boxShadow: '0 20px 46px -14px rgba(20,16,40,0.34)',
          borderLeft: `4px solid ${meta.color}`,
        }}>
        <div className="p-5 space-y-3">
          <div className="flex items-center gap-2.5 flex-wrap">
            <ProviderGlyph provider={task.provider} size={18} />
            <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text">{task.key}</span>
            <span className="mt-chip ml-auto px-2 py-0.5 rounded" style={{ color: meta.color, border: `1px solid ${meta.color}` }}>
              {meta.label}
            </span>
          </div>

          <p className="mt-title-lg text-title">{task.title}</p>

          {issues.length > 0 ? (
            <div className="space-y-2.5">
              {issues.map(it => (
                <FixRow key={it.code} issue={it} task={task}
                  onApplied={() => onApplied(it.code)} onIgnore={() => onIgnore(it.code)} />
              ))}
            </div>
          ) : (
            <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>No recent activity, or no clear signal it&apos;s live.</p>
          )}

          {task.url && (
            <a href={task.url} target="_blank" rel="noopener noreferrer"
              className="mt-body-sm inline-block" style={{ color: 'var(--t-faint)' }}>Open in tracker ↗</a>
          )}
        </div>
      </div>

      <div className="flex items-center gap-3">
        <NavFab glyph="‹" label="Previous ticket" onClick={onPrev} disabled={!hasPrev} />
        {group === 'review' && (
          <button onClick={onKeep}
            className="mt-body-sm px-5 py-2.5 rounded-full transition-transform active:scale-95"
            style={{ background: 'var(--color-state-approved)', color: '#fff' }}>
            Keep
          </button>
        )}
        <NavFab glyph="›" label="Next ticket" onClick={onNext} disabled={!hasNext} />
      </div>
    </div>
  )
}

function NavFab({ glyph, label, onClick, disabled }: {
  glyph: string; label: string; onClick: () => void; disabled: boolean
}) {
  return (
    <button onClick={onClick} disabled={disabled} aria-label={label}
      className="inline-flex items-center justify-center rounded-full transition-transform active:scale-95"
      style={{
        width: 40, height: 40,
        color: disabled ? 'var(--t-faint-2)' : 'var(--t-muted)',
        background: 'var(--t-card)',
        border: '1.5px solid var(--t-hair)',
        opacity: disabled ? 0.5 : 1,
        fontSize: 16,
      }}>
      {glyph}
    </button>
  )
}

type RowState = 'idle' | 'saving' | 'applied' | 'redirected' | 'error'

// One issue + its inline fix control, right on the card — no popup. Apply
// POSTs the value to /api/triage/apply, which runs the daemon write-back
// against the real tracker and reports applied or redirected (open the card).
function FixRow({ issue, task, onApplied, onIgnore }: {
  issue: HygieneIssue; task: TaskSummary; onApplied: () => void; onIgnore: () => void
}) {
  const fix = issue.fix
  const [value, setValue] = useState('')
  const [state, setState] = useState<RowState>('idle')
  const [msg, setMsg] = useState('')
  // Tracks which terminal action button is pending ('close' | 'cancel' | null).
  const [terminalPending, setTerminalPending] = useState<string | null>(null)

  const needsValue = fix?.control !== 'assign_self'
  const canApply = !!fix && state !== 'saving' && state !== 'applied' && (!needsValue || value.trim().length > 0)

  const callApply = async (field: string, val: string) => {
    setState('saving'); setMsg('')
    try {
      const payload = { provider: task.provider, key: task.key, field, value: val }
      // Dual-path: apply_ticket_fix (Rust) in the app, /api/triage/apply in a
      // browser. mutate throws the route's error text on failure → show it.
      const data = await mutate<{ result: { status: string; browse_url?: string; reason?: string } }>(
        '/api/triage/apply', 'apply_ticket_fix', payload)
      const result = data.result
      if (result.status === 'applied') {
        setState('applied'); setTerminalPending(null); setMsg('Saved to tracker')
        onApplied()
      } else {
        setState('redirected'); setTerminalPending(null)
        setMsg(result.reason ?? 'Finish this in your tracker')
        const url = result.browse_url || task.url
        if (url) window.open(url, '_blank', 'noopener')
      }
    } catch (e) {
      // Tauri rejects with a plain string, not an Error object — handle both.
      setState('error'); setTerminalPending(null); setMsg(e instanceof Error ? e.message : typeof e === 'string' ? e : 'Network error')
    }
  }

  // Apply a concrete value for this field (the parent picker passes the
  // parent key directly; the standard controls pass their own input).
  const applyValue = async (val: string) => {
    if (!fix) return
    await callApply(fix.field, fix.control === 'assign_self' ? '@me' : val.trim())
  }

  const applyTerminal = (action: 'close' | 'cancel') => {
    setTerminalPending(action)
    callApply(action, '')
  }

  const apply = () => applyValue(value)

  const applied = state === 'applied'
  const badgeColor = applied ? 'var(--color-state-approved)' : (issue.severity === 'must_fix' ? 'var(--severity-must)' : 'var(--severity-nice)')

  return (
    <div className="rounded-xl p-3.5" style={{
      border: `1px solid ${applied ? 'var(--color-state-approved)' : 'var(--t-card-border)'}`,
      background: applied ? 'color-mix(in srgb, var(--color-state-approved) 8%, transparent)' : 'var(--t-box)',
    }}>
      <div className="flex items-start gap-2.5">
        <span className="shrink-0 inline-block rounded-full mt-1" style={{ width: 7, height: 7, background: badgeColor }} />

        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 mb-1.5">
            <span className="mt-body-sm flex-1 min-w-0" style={{ color: 'var(--t-title)', fontWeight: 600 }}>{issue.hint}</span>
            {issue.severity === 'optional' && !applied && (
              <button onClick={onIgnore} className="mt-body-sm shrink-0" style={{ color: 'var(--t-faint)' }} title="Ignore this for this ticket">
                Ignore
              </button>
            )}
          </div>

          {applied ? (
            <p className="mt-body-sm" style={{ color: 'var(--color-state-approved)', fontWeight: 600 }}>✓ {msg}</p>
          ) : fix?.control === 'pick_parent' ? (
            <>
              <ParentPicker provider={task.provider} taskKey={task.key} saving={state === 'saving'} onPick={applyValue} />
              {(state === 'error' || state === 'redirected') && (
                <p className="mt-body-sm mt-1.5" style={{ color: state === 'error' ? 'var(--severity-must)' : 'var(--t-muted)' }}>{msg}</p>
              )}
            </>
          ) : (
            <>
              <div className="flex items-center gap-2">
                <Control control={fix?.control} value={value} onChange={setValue} />
                <button onClick={apply} disabled={!canApply}
                  className="mt-body-sm px-3 py-1.5 rounded-lg shrink-0"
                  style={{
                    background: canApply ? 'var(--color-state-proposal)' : 'var(--t-hair)',
                    color: canApply ? '#fff' : 'var(--t-faint)',
                    cursor: canApply ? 'pointer' : 'not-allowed',
                  }}>
                  {state === 'saving' && terminalPending === null ? 'Saving…' : (fix?.label ?? 'Fix')}
                </button>
              </div>
              {issue.code === 'overdue' && (
                <div className="flex items-center gap-2 mt-2 pt-2 border-t" style={{ borderColor: 'var(--t-hair)' }}>
                  <span className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>or mark as</span>
                  <button disabled={state === 'saving'} onClick={() => applyTerminal('close')}
                    className="mt-body-sm px-2.5 py-1 rounded-md" style={{ border: '1px solid var(--t-hair)', color: 'var(--t-muted)' }}>
                    {terminalPending === 'close' ? 'Saving…' : 'Done'}
                  </button>
                  <button disabled={state === 'saving'} onClick={() => applyTerminal('cancel')}
                    className="mt-body-sm px-2.5 py-1 rounded-md" style={{ border: '1px solid var(--t-hair)', color: 'var(--t-muted)' }}>
                    {terminalPending === 'cancel' ? 'Saving…' : 'Cancelled'}
                  </button>
                </div>
              )}
              {(state === 'error' || state === 'redirected') && (
                <p className="mt-body-sm mt-2" style={{ color: state === 'error' ? 'var(--severity-must)' : 'var(--t-muted)' }}>{msg}</p>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  )
}

interface Parent { key: string; title: string }

// Real parent picker for the "link to a parent" fix. "Parent" is the level
// above the ticket and is named per tracker (Jira Epic / Azure parent work
// item / Linear·GitHub parent issue) — the daemon returns that label.
function ParentPicker({ provider, taskKey, saving, onPick }: {
  provider: string; taskKey: string; saving: boolean; onPick: (parentKey: string) => void
}) {
  const [parents, setParents] = useState<Parent[] | null>(null)
  const [label, setLabel] = useState('parent')
  const [createUrl, setCreateUrl] = useState('')
  const [query, setQuery] = useState('')
  const [err, setErr] = useState('')

  useEffect(() => {
    let alive = true
    // get_ticket_parents (Rust, shells out to `meridian ticket-parents`) in the
    // Tauri window; /api/triage/parents in a browser.
    load<{ parents?: Parent[]; parent_label?: string; create_url?: string; error?: string }>(
      `/api/triage/parents?provider=${encodeURIComponent(provider)}&key=${encodeURIComponent(taskKey)}`,
      'get_ticket_parents',
      { provider, key: taskKey },
    )
      .then((d) => {
        if (!alive) return
        setParents(d.parents ?? [])
        setLabel(d.parent_label || 'parent')
        setCreateUrl(d.create_url ?? '')
        if (d.error) setErr(d.error)
      })
      .catch(() => { if (alive) { setParents([]); setErr('Could not load parents') } })
    return () => { alive = false }
  }, [provider, taskKey])

  const filtered = (parents ?? []).filter(e =>
    !query || e.key.toLowerCase().includes(query.toLowerCase()) || e.title.toLowerCase().includes(query.toLowerCase()))

  return (
    <div className="space-y-2">
      {parents === null ? (
        <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>Loading {label}s…</p>
      ) : parents.length === 0 ? (
        <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
          {err ? err : `No ${label} to link yet${createUrl ? ' — create one below.' : ' — add one in your tracker.'}`}
        </p>
      ) : (
        <>
          <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>Pick a {label} to link this under:</p>
          {parents.length > 6 && (
            <input type="text" value={query} onChange={e => setQuery(e.target.value)}
              placeholder={`Filter ${label}s…`}
              className="w-full mt-body-sm px-2.5 py-1.5 rounded-md"
              style={{ border: '1px solid var(--t-input-border)', background: 'var(--t-input)', color: 'var(--t-title)' }} />
          )}
          <div className="max-h-36 overflow-auto rounded-md divide-y" style={{ border: '1px solid var(--t-hair)', background: 'var(--t-input)' }}>
            {filtered.map(e => (
              <button key={e.key} disabled={saving} onClick={() => onPick(e.key)}
                className="w-full flex items-center gap-2 px-2.5 py-2 text-left"
                style={{ borderColor: 'var(--t-hair)', cursor: saving ? 'wait' : 'pointer' }}>
                <span className="mt-mono-sm text-[11px] shrink-0" style={{ color: 'var(--color-state-proposal)' }}>{e.key}</span>
                <span className="mt-body-sm truncate" style={{ color: 'var(--t-muted)' }}>{e.title}</span>
              </button>
            ))}
            {filtered.length === 0 && (
              <p className="px-2.5 py-2 mt-body-sm" style={{ color: 'var(--t-faint)' }}>No {label} matches “{query}”.</p>
            )}
          </div>
        </>
      )}

      {createUrl && (
        <a href={createUrl} target="_blank" rel="noopener noreferrer"
          className="inline-flex items-center gap-1 mt-body-sm" style={{ color: 'var(--t-faint)' }}>
          + Create a new {label} in the tracker ↗
        </a>
      )}
    </div>
  )
}

function Control({ control, value, onChange }: { control?: string; value: string; onChange: (v: string) => void }) {
  const base = 'flex-1 min-w-0 mt-body-sm px-2.5 py-1.5 rounded-md'
  const style: React.CSSProperties = { border: '1px solid var(--t-input-border)', background: 'var(--t-input)', color: 'var(--t-title)' }
  switch (control) {
    case 'date_picker':
      return <input type="date" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    case 'assign_self':
      return <span className="flex-1 mt-body-sm" style={{ color: 'var(--t-muted)' }}>Assign this ticket to you.</span>
    case 'pick_priority':
      return (
        <select value={value} onChange={e => onChange(e.target.value)} className={base} style={style}>
          <option value="">Select priority…</option>
          {PRIORITIES.map(p => <option key={p} value={p}>{p}</option>)}
        </select>
      )
    case 'number_input':
      return <input type="number" min={0} placeholder="Story points" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    case 'edit_labels':
      return <input type="text" placeholder="label (e.g. backend)" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    case 'pick_parent':
      return <input type="text" placeholder="Epic / parent key (e.g. KAN-50)" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    case 'edit_text':
    case 'edit_checklist':
      return <textarea rows={2} placeholder="Type here…" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    default:
      return <span className="flex-1" />
  }
}
