//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The "write your own task" surface, used two ways:
//   - as the board column's composer (＋ New task), for people with a tracker;
//   - as the first-run empty state, for people without one — where it replaces the
//     old "Connect a tracker in Settings" dead end. Same component either way.
//
// THE LAYOUT IS THE PRODUCT PROMISE: the note box drafts with AI, but the fields below
// it are present, empty and editable from the first render — never gated behind the
// draft. Typing a title and hitting Create without ever touching the AI is a
// first-class path, not a fallback. See useTaskComposer's header.
//
// Colour follows STYLESHEET §9's three-hue budget: violet = AI-authored (the Draft
// button, the "Drafted" chips), green = a commit action (Create). The shimmer +
// --gen-* tokens are §9.2's reserved "a model is generating right now" treatment,
// which is exactly what's happening.

import { useEffect, useRef } from 'react'
import type { Tracker } from '@/lib/integrations'
import { LOCAL_PROVIDER } from '@/lib/api-types'
import {
  useTaskComposer, setNote, setTitle, setDescription, setIssueType, setTarget,
  draftFromNote, createTask, canCreate, resetComposer, titleWords, MIN_TITLE_WORDS,
} from '@/components/plan/useTaskComposer'
import { FOCUS, PRIMARY_BTN } from '@/components/plan/planStyles'
import { GeneratingBar } from '@/components/GeneratingBar'

const INPUT = 'w-full px-3 py-2 rounded-lg mt-body'
const inputStyle = {
  background: 'var(--t-input)',
  border: '1px solid var(--t-input-border)',
  color: 'var(--t-title)',
  outline: 'none',
} as const

/** A field label + its "Drafted" chip. The chip is the honesty marker: it says a model
 *  wrote this, check it. It clears the moment the user edits (the store owns that). */
function FieldLabel({ children, drafted }: { children: React.ReactNode; drafted?: boolean }) {
  return (
    <div className="flex items-center gap-1.5 mb-1">
      <label className="mt-label" style={{ color: 'var(--t-faint)' }}>{children}</label>
      {drafted && (
        <span className="mt-chip px-1.5 py-0.5 rounded"
          style={{ color: 'var(--color-state-proposal)', background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' }}>
          Drafted
        </span>
      )}
    </div>
  )
}

export function TaskComposer({ day, trackers, onDone, onCancel, hero = false }: {
  day: string
  /** Connected trackers. Empty = solo: no sync choice to offer at all. */
  trackers: Tracker[]
  /** A task was created and added to the plan. */
  onDone: () => void
  /** Only rendered when provided (the board column's back affordance). */
  onCancel?: () => void
  /** First-run empty state: centred, elevated, with a headline. */
  hero?: boolean
}) {
  const s = useTaskComposer()
  const noteRef = useRef<HTMLTextAreaElement>(null)

  // A fresh composer every time it opens — a stale half-typed task from an hour ago is
  // never what the user wants back. (The store's job is surviving an unmount DURING a
  // flow, not resurrecting an abandoned one.)
  useEffect(() => {
    resetComposer()
    noteRef.current?.focus()
  }, [])

  // The create landed — let the parent close/refresh. Kept as an effect so the store,
  // not the click handler, is the single source of "it worked".
  useEffect(() => {
    if (s.created) onDone()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.created])

  const drafting = s.phase === 'drafting'
  const creating = s.phase === 'creating'
  const ready = canCreate(s)
  const words = titleWords(s.title)

  return (
    <div className={hero ? 'w-full max-w-[560px] mx-auto' : 'flex flex-col min-h-0 h-full'}>
      {onCancel && (
        <button onClick={onCancel} className={`mt-body-sm inline-flex items-center gap-1.5 mb-3 self-start ${FOCUS}`}
          style={{ color: 'var(--t-faint)', fontWeight: 700 }}>
          <span aria-hidden>‹</span> Back to your tasks
        </button>
      )}

      <div className={hero ? 'rounded-2xl bg-card p-6' : 'flex-1 min-h-0 overflow-y-auto nice-scroll'}
        style={hero ? { border: '1px solid var(--t-card-border)', boxShadow: 'var(--mt-card-shadow)' } : undefined}>
        {hero && (
          <div className="mb-4">
            <p className="mt-title" style={{ color: 'var(--t-title)' }}>What are you working on today?</p>
            <p className="mt-body-sm mt-1.5" style={{ color: 'var(--t-faint)' }}>
              Jot it down however it comes out - we&apos;ll shape it into a task. Or just fill the fields in yourself.
            </p>
          </div>
        )}

        {/* ── The note → AI draft ─────────────────────────────────────────── */}
        <FieldLabel>{hero ? 'Your note' : 'What are you working on?'}</FieldLabel>
        <textarea
          ref={noteRef} rows={2} value={s.note} onChange={e => setNote(e.target.value)}
          disabled={drafting}
          placeholder="fix the flaky login test…"
          className={`${INPUT} ${FOCUS}`}
          style={{ ...inputStyle, resize: 'vertical', opacity: drafting ? 0.6 : 1 }}
        />
        <div className="flex justify-end mt-2">
          <button onClick={draftFromNote} disabled={drafting || !s.note.trim()}
            className={`mt-body-sm inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg ${FOCUS}`}
            style={{
              fontWeight: 700, color: '#fff', background: 'var(--color-state-proposal)',
              opacity: drafting || !s.note.trim() ? 0.5 : 1,
              cursor: drafting || !s.note.trim() ? 'default' : 'pointer',
              boxShadow: '0 8px 22px -10px var(--color-state-proposal)',
            }}>
            <span aria-hidden>✨</span> {drafting ? 'Drafting…' : 'Draft it'}
          </button>
        </div>

        {drafting && (
          <div className="mt-3">
            <GeneratingBar
              hue="var(--color-state-proposal)"
              label="Drafting your task…"
              detail="Shaping your note into a title and description - they land in the fields below, yours to edit."
              note="this usually takes a few seconds"
            />
          </div>
        )}

        {s.error && (
          <p className="mt-body-sm mt-2 rounded-lg px-3 py-2"
            style={{ color: 'var(--color-state-pending)', background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)' }}>
            {s.error}
          </p>
        )}

        <div className="my-4 border-t" style={{ borderColor: 'var(--t-hair)' }} />

        {/* ── The task itself — ALWAYS editable, never gated on the draft ──── */}
        <div className={`rounded-xl ${drafting ? 'mer-gen-shimmer p-3 -m-0.5' : ''}`}
          style={drafting ? {
            border: '1px solid var(--gen-border)',
            background: 'linear-gradient(100deg, var(--gen-bg-1), var(--gen-bg-2), var(--gen-bg-1))',
            backgroundSize: '200% 100%',
          } : undefined}>
          <FieldLabel drafted={s.titleDrafted}>Title</FieldLabel>
          <input
            value={s.title} onChange={e => setTitle(e.target.value)}
            placeholder="What needs doing?"
            className={`${INPUT} ${FOCUS}`} style={inputStyle}
          />

          <div className="mt-3">
            <FieldLabel drafted={s.descriptionDrafted}>Description</FieldLabel>
            <textarea
              rows={3} value={s.description} onChange={e => setDescription(e.target.value)}
              placeholder="Briefly, what the task is - name the thing you're working on."
              className={`${INPUT} ${FOCUS}`} style={{ ...inputStyle, resize: 'vertical' }}
            />
          </div>

          <div className="mt-3 flex items-center gap-2">
            <label className="mt-label" style={{ color: 'var(--t-faint)' }}>Type</label>
            <div role="radiogroup" aria-label="Task type" className="flex rounded-md overflow-hidden"
              style={{ border: '1px solid var(--t-ctrl-border)' }}>
              {['Task', 'Bug'].map(t => (
                <button key={t} role="radio" aria-checked={s.issueType === t} onClick={() => setIssueType(t)}
                  className={`mt-chip px-2.5 py-1.5 transition-colors ${FOCUS}`}
                  style={{
                    background: s.issueType === t ? 'var(--t-wrap)' : 'var(--t-ctrl)',
                    color: s.issueType === t ? 'var(--t-title)' : 'var(--t-faint)',
                  }}>
                  {t}
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* ── Where it goes. Solo users have no choice to make, so none is shown.
             Every connected tracker gets its own option - someone on Jira AND
             GitHub must be able to reach both, not just whichever sorts first. */}
        {trackers.length > 0 && (
          <div className="mt-4">
            <FieldLabel>Where</FieldLabel>
            <div role="radiogroup" aria-label="Where to create this task"
              className={`grid gap-2 ${trackers.length > 1 ? 'grid-cols-1 sm:grid-cols-2' : 'grid-cols-2'}`}>
              <TargetOption
                selected={s.target === LOCAL_PROVIDER} onSelect={() => setTarget(LOCAL_PROVIDER)}
                label="Keep personal" hint="Only in Meridian. Nothing is posted to your board."
              />
              {trackers.map(t => (
                <TargetOption key={t.id}
                  selected={s.target === t.id} onSelect={() => setTarget(t.id)}
                  label={`Create in ${t.name}`} hint={`Files a real ${t.name} ticket your team can see.`}
                />
              ))}
            </div>
          </div>
        )}
      </div>

      {/* ── Commit. Green = you committed it (violet is reserved for AI). ───── */}
      <div className={hero ? 'mt-4' : 'shrink-0 pt-3 mt-3 border-t'}
        style={hero ? undefined : { borderColor: 'var(--t-hair)' }}>
        <button onClick={() => createTask(day)} disabled={!ready}
          className={`w-full ${PRIMARY_BTN} ${FOCUS}`}
          style={{
            background: 'var(--color-state-approved)', color: '#fff',
            opacity: ready ? 1 : 0.5, cursor: ready ? 'pointer' : 'default',
          }}>
          {creating ? 'Adding…' : 'Add to today →'}
        </button>
        {/* Says WHY, and only names the rule you're currently short of - a form that
            barks every requirement at an empty box is noise. See MIN_TITLE_WORDS. */}
        {!s.title.trim() && !s.description.trim() ? (
          <p className="mt-2 text-center mt-body-sm" style={{ color: 'var(--t-faint-2)' }}>
            A title and a line on what you&apos;re doing.
          </p>
        ) : words < MIN_TITLE_WORDS ? (
          <p className="mt-2 text-center mt-body-sm" style={{ color: 'var(--color-state-pending)' }}>
            {MIN_TITLE_WORDS - words === 1 ? 'One more word' : `${MIN_TITLE_WORDS - words} more words`} - name the work, not just the subject.
          </p>
        ) : !s.description.trim() ? (
          <p className="mt-2 text-center mt-body-sm" style={{ color: 'var(--color-state-pending)' }}>
            Describe the task - this is what Meridian matches your work against.
          </p>
        ) : null}
      </div>
    </div>
  )
}

/** One "where does this go" choice. A radio, not a toggle - both options are equally
 *  legible, because filing on a shared board is a decision worth reading first. */
function TargetOption({ selected, onSelect, label, hint }: {
  selected: boolean; onSelect: () => void; label: string; hint: string
}) {
  return (
    <button role="radio" aria-checked={selected} onClick={onSelect}
      className={`text-left rounded-lg px-3 py-2.5 transition-colors ${FOCUS}`}
      style={{
        border: `1px solid ${selected ? 'var(--color-state-approved)' : 'var(--t-ctrl-border)'}`,
        background: selected ? 'color-mix(in srgb, var(--color-state-approved) 8%, transparent)' : 'var(--t-ctrl)',
      }}>
      <span className="mt-body-sm block" style={{ fontWeight: 700, color: 'var(--t-title)' }}>{label}</span>
      <span className="mt-body-sm block mt-0.5" style={{ color: 'var(--t-faint)', fontSize: 11.5, lineHeight: 1.4 }}>{hint}</span>
    </button>
  )
}
