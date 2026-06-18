//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState } from 'react'
import { TaskKey, ProviderGlyph, StatusPill } from '@/components/atoms'
import type { TaskSummary } from '@/lib/api-types'
import type { HygieneIssue } from '@/lib/hygiene'
import { load, mutate } from '@/lib/bridge'

const PRIORITIES = ['Highest', 'High', 'Medium', 'Low', 'Lowest']

// A focused "fix this ticket" dialog: each Definition-of-Ready defect with the
// right input control. Apply writes the fix straight to the user's tracker via
// /api/triage/apply; fields a provider can't write land as a redirect to the card.
export default function HygieneDialog({ task, onClose, onApplied }: {
  task: TaskSummary
  onClose: () => void
  onApplied?: () => void
}) {
  const issues = task.hygiene?.issues ?? []

  // Close on Escape.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const done = issues.length === 0

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 rise"
      style={{ background: 'rgba(20,16,10,0.45)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
      <div className="w-full max-w-2xl rounded-[20px] overflow-hidden flex flex-col max-h-[88vh]"
        style={{ background: 'var(--paper)', border: '1px solid var(--rule)', boxShadow: '0 24px 60px -12px rgba(20,16,10,0.35)' }}
        onClick={e => e.stopPropagation()}>
        {/* Accent hairline */}
        <div style={{ height: 3, background: 'linear-gradient(90deg, var(--accent), var(--warn))' }} />

        {/* Header band */}
        <div className="px-7 pt-6 pb-5" style={{ background: 'var(--tint)', borderBottom: '1px solid var(--rule)' }}>
          <div className="flex items-center gap-2.5 mb-3">
            <ProviderGlyph provider={task.provider} size={18} />
            <TaskKey keyId={task.key} />
            <StatusPill status={task.status} isTerminal={task.is_terminal} />
            <button onClick={onClose} aria-label="Close"
              className="ml-auto inline-flex items-center justify-center rounded-full transition-colors"
              style={{ width: 28, height: 28, color: 'var(--ink-3)', background: 'var(--surface)', border: '1px solid var(--rule)' }}>
              <span className="text-[16px] leading-none">×</span>
            </button>
          </div>
          <h2 className="type-heading leading-snug" style={{ color: 'var(--ink)' }}>{task.title}</h2>
          <div className="flex items-center gap-2 mt-3">
            <span className="inline-flex items-center gap-1.5 text-[11px] px-2.5 py-1 rounded-full font-medium"
              style={ done
                ? { background: 'var(--success)' + '1A', color: 'var(--success)' }
                : { background: 'var(--warn)' + '1A', color: 'var(--warn)' }}>
              <span style={{ fontSize: 11 }}>{done ? '✓' : '⚠'}</span>
              {done ? 'Looks complete' : `${issues.length} fix${issues.length === 1 ? '' : 'es'} to tidy`}
            </span>
            <span className="text-[12px]" style={{ color: 'var(--ink-3)' }}>
              {done ? 'Review it, or close it in your tracker.' : 'to make this a well-formed ticket'}
            </span>
          </div>
        </div>

        {/* Fix rows */}
        <div className="px-7 py-6 space-y-3.5 overflow-auto" style={{ background: 'var(--paper)' }}>
          {done ? (
            <div className="py-10 text-center rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
              <p className="text-[13px]" style={{ color: 'var(--ink-2)' }}>Nothing to fix here.</p>
              <p className="text-[12px] mt-1" style={{ color: 'var(--ink-4)' }}>This ticket either looks stale or already has what Meridian needs.</p>
            </div>
          ) : issues.map((it, i) => <FixRow key={it.code} issue={it} index={i + 1} task={task} onApplied={onApplied} />)}
        </div>

        {/* Footer */}
        <div className="px-7 py-4 flex items-center gap-3" style={{ borderTop: '1px solid var(--rule)', background: 'var(--surface)' }}>
          <p className="text-[11px] flex-1" style={{ color: 'var(--ink-4)' }}>
            Fixes save straight to your tracker. AI suggestions for text fields are coming.
          </p>
          {task.url && (
            <a href={task.url} target="_blank" rel="noopener noreferrer"
              className="text-[12px] px-3.5 py-2 rounded-lg border transition-colors"
              style={{ borderColor: 'var(--rule)', color: 'var(--ink-2)', background: 'var(--paper)' }}>
              Open in tracker ↗
            </a>
          )}
        </div>
      </div>
    </div>
  )
}

type RowState = 'idle' | 'saving' | 'applied' | 'redirected' | 'error'

// One defect + its control. Apply POSTs the value to /api/triage/apply, which
// runs the daemon write-back against the real tracker and reports applied or
// redirected (open the card).
function FixRow({ issue, index, task, onApplied }: { issue: HygieneIssue; index: number; task: TaskSummary; onApplied?: () => void }) {
  const fix = issue.fix
  const [value, setValue] = useState('')
  const [state, setState] = useState<RowState>('idle')
  const [msg, setMsg] = useState('')

  const needsValue = fix?.control !== 'assign_self'
  const canApply = !!fix && state !== 'saving' && state !== 'applied' && (!needsValue || value.trim().length > 0)

  // Apply a concrete value for this field (the epic picker passes the epic key
  // directly; the standard controls pass their own input).
  const applyValue = async (val: string) => {
    if (!fix) return
    setState('saving'); setMsg('')
    try {
      const payload = {
        provider: task.provider,
        key: task.key,
        field: fix.field,
        value: fix.control === 'assign_self' ? '@me' : val.trim(),
      }
      // Dual-path: apply_ticket_fix (Rust) in the app, /api/triage/apply in a
      // browser. mutate throws the route's error text on failure → show it.
      const data = await mutate<{ result: { status: string; browse_url?: string; reason?: string } }>(
        '/api/triage/apply', 'apply_ticket_fix', payload)
      const result = data.result
      if (result.status === 'applied') {
        setState('applied'); setMsg('Saved to tracker')
        onApplied?.()
      } else {
        // Provider can't write this field — open the card so the dev finishes there.
        setState('redirected')
        setMsg(result.reason ?? 'Finish this in your tracker')
        const url = result.browse_url || task.url
        if (url) window.open(url, '_blank', 'noopener')
      }
    } catch (e) {
      setState('error'); setMsg(e instanceof Error ? e.message : 'Network error')
    }
  }

  const apply = () => applyValue(value)

  const applied = state === 'applied'
  const badgeColor = applied ? 'var(--success)' : (issue.severity === 'must_fix' ? 'var(--warn)' : 'var(--accent)')

  return (
    <div className="rounded-2xl border p-4 transition-colors"
      style={{ borderColor: applied ? 'var(--success)' + '55' : 'var(--rule)', background: applied ? 'var(--success)' + '0C' : 'var(--surface)' }}>
      <div className="flex items-start gap-3">
        {/* Leading status badge */}
        <span className="shrink-0 inline-flex items-center justify-center rounded-full text-[12px] font-medium mt-0.5"
          style={{ width: 24, height: 24, background: badgeColor + '1A', color: badgeColor }}>
          {applied ? '✓' : index}
        </span>

        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 mb-2.5">
            <span className="text-[13px] font-medium" style={{ color: 'var(--ink)' }}>{issue.hint}</span>
            {issue.severity === 'must_fix' && !applied && (
              <span className="text-[10px] px-1.5 py-0.5 rounded-full font-medium"
                style={{ background: 'var(--warn)' + '1A', color: 'var(--warn)' }}>Required</span>
            )}
            {fix?.ai && !applied && (
              <span className="ml-auto text-[10px] px-1.5 py-0.5 rounded-full"
                style={{ background: 'var(--accent)' + '1A', color: 'var(--accent)' }}>✨ AI soon</span>
            )}
          </div>

      {applied ? (
        <p className="text-[12px] font-medium" style={{ color: 'var(--success)' }}>✓ {msg}</p>
      ) : fix?.control === 'pick_parent' ? (
        <>
          <ParentPicker provider={task.provider} taskKey={task.key} saving={state === 'saving'} onPick={applyValue} />
          {(state === 'error' || state === 'redirected') && (
            <p className="text-[11px] mt-1.5" style={{ color: state === 'error' ? 'var(--warn)' : 'var(--ink-3)' }}>{msg}</p>
          )}
        </>
      ) : (
        <>
          <div className="flex items-center gap-2">
            <Control control={fix?.control} value={value} onChange={setValue} />
            <button onClick={apply} disabled={!canApply}
              className="text-[12px] font-medium px-3.5 py-2 rounded-lg shrink-0 transition-colors"
              style={{
                background: canApply ? 'var(--accent)' : 'var(--rule)',
                color: canApply ? '#fff' : 'var(--ink-4)',
                cursor: canApply ? 'pointer' : 'not-allowed',
              }}>
              {state === 'saving' ? 'Saving…' : (fix?.label ?? 'Fix')}
            </button>
          </div>
          {(state === 'error' || state === 'redirected') && (
            <p className="text-[11px] mt-2" style={{ color: state === 'error' ? 'var(--warn)' : 'var(--ink-3)' }}>{msg}</p>
          )}
        </>
      )}
        </div>
      </div>
    </div>
  )
}

interface Parent { key: string; title: string }

// Real parent picker for the "link to a parent" fix. "Parent" is the level above
// the ticket and is named per tracker (Jira Epic / parent task, Azure parent work
// item, Linear/GitHub parent issue) — the daemon returns that label. Click a
// parent → set it via the normal write-back; "Create new …" deep-links straight to
// the tracker's create page (creating a parent is a multi-field flow we don't own,
// so it redirects, as the user asked).
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
    // Tauri window; /api/triage/parents in a browser. Same shape — both relay
    // the CLI's JSON and carry an `error` field on failure rather than throwing.
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
        <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>Loading {label}s…</p>
      ) : parents.length === 0 ? (
        <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>
          {err ? err : `No ${label} to link yet${createUrl ? ' — create one below.' : ' — add one in your tracker.'}`}
        </p>
      ) : (
        <>
          <p className="text-[11px]" style={{ color: 'var(--ink-4)' }}>Pick a {label} to link this under:</p>
          {parents.length > 6 && (
            <input type="text" value={query} onChange={e => setQuery(e.target.value)}
              placeholder={`Filter ${label}s…`}
              className="w-full text-[12px] px-2.5 py-1.5 rounded-md border"
              style={{ borderColor: 'var(--rule)', background: 'var(--paper)', color: 'var(--ink)' }} />
          )}
          <div className="max-h-44 overflow-auto rounded-md border divide-y" style={{ borderColor: 'var(--rule)', background: 'var(--paper)' }}>
            {filtered.map(e => (
              <button key={e.key} disabled={saving} onClick={() => onPick(e.key)}
                className="w-full flex items-center gap-2 px-2.5 py-2 text-left transition-colors hover:opacity-80"
                style={{ cursor: saving ? 'wait' : 'pointer', borderColor: 'var(--rule)' }}>
                <span className="font-mono text-[11px] shrink-0" style={{ color: 'var(--accent)' }}>{e.key}</span>
                <span className="text-[12px] truncate" style={{ color: 'var(--ink-2)' }}>{e.title}</span>
              </button>
            ))}
            {filtered.length === 0 && (
              <p className="px-2.5 py-2 text-[11px]" style={{ color: 'var(--ink-4)' }}>No {label} matches “{query}”.</p>
            )}
          </div>
        </>
      )}

      {createUrl && (
        <a href={createUrl} target="_blank" rel="noopener noreferrer"
          className="inline-flex items-center gap-1 text-[11px]" style={{ color: 'var(--ink-3)' }}>
          + Create a new {label} in the tracker ↗
        </a>
      )}
    </div>
  )
}

function Control({ control, value, onChange }: { control?: string; value: string; onChange: (v: string) => void }) {
  const base = 'flex-1 min-w-0 text-[13px] px-2.5 py-1.5 rounded-md border'
  const style: React.CSSProperties = { borderColor: 'var(--rule)', background: 'var(--paper)', color: 'var(--ink)' }
  switch (control) {
    case 'date_picker':
      return <input type="date" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    case 'assign_self':
      return <span className="flex-1 text-[12px]" style={{ color: 'var(--ink-3)' }}>Assign this ticket to you.</span>
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
      return <textarea rows={2} placeholder="Type, or use AI (soon)…" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    default:
      return <span className="flex-1" />
  }
}
