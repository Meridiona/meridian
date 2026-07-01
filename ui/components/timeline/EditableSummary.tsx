//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared inline textarea edit/save/cancel, parameterized so it covers both a
// worklog's summary and a proposed ticket's body/title. Used by
// WorklogDetailPane and ReviewCard — previously this logic was duplicated once
// inline in WorklogCard and once (title + body, two copies) in ProposedCard.

'use client'

import { useEffect, useState } from 'react'

export function EditableSummary({
  label, value, placeholder, busy, rows = 4, onSave,
}: {
  label?: string
  value: string
  placeholder: string
  busy: boolean
  rows?: number
  onSave: (next: string) => void
}) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(value)

  useEffect(() => { setDraft(value) }, [value])

  return (
    <div>
      {label && <p className="mt-label mb-1" style={{ color: 'var(--t-faint)' }}>{label}</p>}
      {editing ? (
        <div>
          <textarea value={draft} onChange={e => setDraft(e.target.value)} rows={rows} disabled={busy}
            className="w-full px-3 py-2 rounded-md mt-body"
            style={{ background: 'var(--t-input)', border: '1px solid var(--t-input-border)', color: 'var(--t-title)', outline: 'none', resize: 'vertical' }} />
          <div className="flex items-center gap-2 mt-2">
            <button onClick={() => { onSave(draft); setEditing(false) }} disabled={busy}
              className="mt-body-sm px-3 py-1 rounded-md" style={{ background: 'var(--t-title)', color: 'var(--t-panel)' }}>Save</button>
            <button onClick={() => { setDraft(value); setEditing(false) }} disabled={busy}
              className="mt-body-sm px-3 py-1 rounded-md" style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>Cancel</button>
          </div>
        </div>
      ) : (
        <p onClick={() => setEditing(true)} className="mt-body whitespace-pre-wrap cursor-text"
          style={{ color: value ? 'var(--t-title)' : 'var(--t-faint)' }}>
          {value || placeholder}
        </p>
      )}
    </div>
  )
}

export function EditableTitle({
  value, busy, onSave,
}: {
  value: string
  busy: boolean
  onSave: (next: string) => void
}) {
  const [title, setTitle] = useState(value)
  useEffect(() => { setTitle(value) }, [value])
  const dirty = title.trim() !== value.trim() && title.trim().length > 0

  return (
    <div>
      <label className="mt-label" style={{ color: 'var(--t-faint)' }}>Ticket title</label>
      <input value={title} onChange={e => setTitle(e.target.value)} disabled={busy}
        className="w-full mt-1 px-2 py-1.5 rounded-md mt-body"
        style={{ color: 'var(--t-title)', background: 'var(--t-input)', border: '1px solid var(--t-input-border)' }} />
      {dirty && (
        <button onClick={() => onSave(title.trim())} disabled={busy}
          className="mt-1 px-2 py-0.5 rounded mt-body-sm" style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
          Save title
        </button>
      )}
    </div>
  )
}
