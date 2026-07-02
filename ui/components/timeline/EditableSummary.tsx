//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared inline textarea edit/save/cancel, parameterized so it covers both a
// worklog's summary and a proposed ticket's body/title. Used by
// WorklogDetailPane and ReviewCard — previously this logic was duplicated once
// inline in WorklogCard and once (title + body, two copies) in ProposedCard.
//
// This is a CONTROLLED edit view, not a click-to-edit widget: callers already
// decide whether to mount it at all (each wraps it in its own
// `editing ? <EditableSummary/> : <staticText/>`), so it renders the
// textarea + Save/Cancel immediately on mount rather than tracking its own
// separate "am I editing" flag. An earlier version had a second, internal
// editing toggle defaulting to false — mounting it left the card showing
// static clickable text with no visible Save button until clicked a second
// time, which read as "editing is broken." Always render editable; let the
// parent's editing state (and its onCancel) be the single source of truth.

'use client'

import { useEffect, useState } from 'react'

export function EditableSummary({
  label, value, placeholder, busy, rows = 4, onSave, onCancel, saveLabel = 'Save',
}: {
  label?: string
  value: string
  placeholder: string
  busy: boolean
  rows?: number
  onSave: (next: string) => void
  onCancel: () => void
  // ReviewCard passes "Save & Approve" for a still-pending draft — Save
  // commits the edit AND approves it in one action rather than a separate
  // second step (see ReviewOverlay's onEditSave).
  saveLabel?: string
}) {
  const [draft, setDraft] = useState(value)

  useEffect(() => { setDraft(value) }, [value])

  return (
    <div>
      {label && <p className="mt-label mb-1" style={{ color: 'var(--t-faint)' }}>{label}</p>}
      <textarea value={draft} onChange={e => setDraft(e.target.value)} rows={rows} disabled={busy}
        placeholder={placeholder}
        className="w-full px-3 py-2 rounded-md mt-body"
        style={{ background: 'var(--t-input)', border: '1px solid var(--t-input-border)', color: 'var(--t-title)', outline: 'none', resize: 'vertical' }} />
      <div className="flex items-center gap-2 mt-2">
        <button onClick={() => onSave(draft)} disabled={busy}
          className="mt-body-sm px-3 py-1 rounded-md" style={{ background: 'var(--t-title)', color: 'var(--t-panel)' }}>{saveLabel}</button>
        <button onClick={onCancel} disabled={busy}
          className="mt-body-sm px-3 py-1 rounded-md" style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>Cancel</button>
      </div>
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
