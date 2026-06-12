//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState } from 'react'
import { TaskKey, ProviderGlyph } from '@/components/atoms'
import type { TaskSummary } from '@/app/api/tasks/route'
import type { HygieneIssue } from '@/lib/hygiene'

const PRIORITIES = ['Highest', 'High', 'Medium', 'Low', 'Lowest']

// A focused "fix this ticket" dialog: each Definition-of-Ready defect with the
// right input control. Apply writes back to the tracker (wired next); for now the
// footer opens the ticket so the dev can apply there.
export default function HygieneDialog({ task, onClose }: { task: TaskSummary; onClose: () => void }) {
  const issues = task.hygiene?.issues ?? []

  // Close on Escape.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,0.4)' }} onClick={onClose}>
      <div className="w-full max-w-lg rounded-2xl border max-h-[85vh] overflow-auto"
        style={{ background: 'var(--paper)', borderColor: 'var(--rule)' }}
        onClick={e => e.stopPropagation()}>
        {/* Header */}
        <div className="px-5 py-4 rule-b sticky top-0" style={{ borderColor: 'var(--rule)', background: 'var(--paper)' }}>
          <div className="flex items-center gap-2 mb-1.5">
            <ProviderGlyph provider={task.provider} size={16} />
            <TaskKey keyId={task.key} />
            <button onClick={onClose} className="ml-auto text-[18px] leading-none" style={{ color: 'var(--ink-3)' }}>×</button>
          </div>
          <h2 className="text-[15px] font-medium" style={{ color: 'var(--ink)' }}>{task.title}</h2>
          <p className="text-[12px] mt-1" style={{ color: 'var(--ink-3)' }}>
            {issues.length} fix{issues.length === 1 ? '' : 'es'} to make this a well-formed ticket.
          </p>
        </div>

        {/* Fix rows */}
        <div className="px-5 py-4 space-y-4">
          {issues.map(it => <FixRow key={it.code} issue={it} task={task} />)}
        </div>

        {/* Footer */}
        <div className="px-5 py-3 rule-t sticky bottom-0 flex items-center gap-3"
          style={{ borderColor: 'var(--rule)', background: 'var(--tint)' }}>
          <p className="text-[11px] flex-1" style={{ color: 'var(--ink-4)' }}>
            One-click in-app save is coming. For now, Apply opens the ticket.
          </p>
          {task.url && (
            <a href={task.url} target="_blank" rel="noopener noreferrer"
              className="text-[12px] px-3 py-1.5 rounded-md"
              style={{ background: 'var(--accent)', color: '#fff' }}>
              Open in tracker ↗
            </a>
          )}
        </div>
      </div>
    </div>
  )
}

// One defect + its input control. The value is collected now; the write-back that
// persists it to the tracker is the next slice (Apply opens the ticket meanwhile).
function FixRow({ issue, task }: { issue: HygieneIssue; task: TaskSummary }) {
  const fix = issue.fix
  const [value, setValue] = useState('')
  const apply = () => { if (task.url) window.open(task.url, '_blank', 'noopener') }

  return (
    <div className="rounded-xl border p-3" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
      <div className="flex items-center gap-2 mb-2">
        <span style={{ color: 'var(--warn)' }}>⚠</span>
        <span className="text-[12px]" style={{ color: 'var(--ink-2)' }}>{issue.hint}</span>
        {fix?.ai && (
          <span className="ml-auto text-[10px] px-1.5 py-0.5 rounded-full"
            style={{ background: 'var(--accent)' + '1A', color: 'var(--accent)' }}>✨ AI soon</span>
        )}
      </div>
      <div className="flex items-center gap-2">
        <Control control={fix?.control} value={value} onChange={setValue} />
        <button onClick={apply}
          className="text-[12px] px-2.5 py-1.5 rounded-md border shrink-0"
          style={{ borderColor: 'var(--rule)', color: 'var(--ink-2)' }}>
          {fix?.label ?? 'Fix'} ↗
        </button>
      </div>
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
      return <input type="text" placeholder="label-one, label-two" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    case 'pick_parent':
      return <input type="text" placeholder="Epic / parent key (e.g. KAN-50)" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    case 'edit_text':
    case 'edit_checklist':
      return <textarea rows={2} placeholder="Type, or use AI (soon)…" value={value} onChange={e => onChange(e.target.value)} className={base} style={style} />
    default:
      return <span className="flex-1" />
  }
}
