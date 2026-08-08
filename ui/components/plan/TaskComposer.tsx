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

import { useCallback, useEffect, useRef, useState } from 'react'
import type { Tracker } from '@/lib/integrations'
import { LOCAL_PROVIDER, type HealthStatus } from '@/lib/api-types'
import { load } from '@/lib/bridge'
import {
  useTaskComposer, setNote, setTitle, setDescription, setIssueType, setTarget,
  draftFromNote, createTask, canCreate, resetComposer, consumeResume, titleWords, MIN_TITLE_WORDS,
  boardProviderFor, isBoardTarget,
} from '@/components/plan/useTaskComposer'
import { FOCUS, PRIMARY_BTN } from '@/components/plan/planStyles'
import { AiEngineNotice } from '@/components/plan/AiEngineNotice'
import { GeneratingBar } from '@/components/GeneratingBar'
import { ProviderIcon } from '@/components/ProviderIcon'

/** Floor on the "checking your AI" beat — see `draftOrConnect`. */
const CHECK_MIN_MS = 750
const wait = (ms: number) => new Promise<void>(r => setTimeout(r, ms))

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
          style={{ color: 'var(--t-accent)', background: 'color-mix(in srgb, var(--t-accent) 12%, transparent)' }}>
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
  //
  // The ONE exception is a resume: pressing "Draft with AI" with no provider sends the user
  // to Settings, which takes the shell's single modal slot and unmounts this. Coming back
  // is the same flow continuing, not a new open — so the note is kept and the draft they
  // already asked for runs on its own. Without this the reset below wiped the note and the
  // user was returned to an empty box having been promised otherwise.
  // ASKED ONCE PER MOUNT, not once per effect RUN. `consumeResume` is one-shot by design,
  // and under `reactStrictMode` (next.config.ts) React deliberately runs this effect, tears
  // it down, and runs it AGAIN on the same instance. The second run found the flag already
  // spent, read that as "an ordinary open", and fell through to `resetComposer()` - so the
  // exact flow this branch exists to protect wiped the note it was protecting and never
  // started the draft. The user connected a provider and landed on an empty composer.
  //
  // The ref caches the answer for the life of the instance (it survives the StrictMode
  // remount, which is the whole point), so both runs take the same branch and the second
  // one re-arms the draft timer the first one's cleanup cancelled.
  const resuming = useRef<boolean | null>(null)
  useEffect(() => {
    if (resuming.current === null) resuming.current = consumeResume()
    if (resuming.current) {
      // Whatever the health probe last thought is stale by definition - they just
      // connected one. Clear the notice so it cannot flash back over the fields.
      setNoProvider(false)
      setShowAiNotice(false)
      // A beat, so the planner has visibly reappeared before it starts doing things by
      // itself. Drafting in the same frame as the modal opens reads as a glitch rather
      // than as the app picking up where it left off.
      const t = setTimeout(() => { if (s.note.trim()) draftFromNote() }, 650)
      return () => clearTimeout(t)
    }
    resetComposer()
    noteRef.current?.focus()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // A tracker can be disconnected in Settings while this composer sits open, stranding
  // `target` on a provider that is no longer connected — the create would then fail at
  // the daemon with nothing the user could have seen coming. Re-point at the surviving
  // board choice; if none survives, fall back to Personal rather than filing blind.
  useEffect(() => {
    if (!isBoardTarget(s)) return
    if (trackers.some(t => t.id === s.target)) return
    setTarget(boardProviderFor(s, trackers.map(t => t.id)) ?? LOCAL_PROVIDER)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.target, trackers])

  // The create landed — let the parent close/refresh. Kept as an effect so the store,
  // not the click handler, is the single source of "it worked".
  useEffect(() => {
    if (s.created) onDone()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.created])

  // Can "Draft with AI" work at all? On a fresh install nobody has connected a
  // provider yet, and the draft call would come back as a red error string under
  // the button — a dead end at the exact moment a new user is being asked to
  // trust the feature.
  //
  // "Connected" is APP state, not what happens to be on the disk: the provider
  // the user picked in Settings, plus whether its last real test passed. That is
  // exactly the ladder `classify_provider_health` already runs (not installed, or
  // last test Failed → `ok: false`), surfaced as `get_health`'s `llm_provider_ok`
  // — so this reuses it rather than inventing a second, disagreeing definition of
  // connected. An earlier version asked `detect_llm_providers` whether ANY CLI
  // existed anywhere, which called a signed-out user connected because the binary
  // was on their machine.
  const [noProvider, setNoProvider] = useState(false)
  const probeProviders = useCallback(async () => {
    try {
      const h = await load<HealthStatus>('/api/health', 'get_health')
      const bad = h?.llm_provider_ok === false
      setNoProvider(bad)
      return !bad
    } catch {
      // Probe failed - assume it works. Blocking the button on a failed probe
      // would break drafting for reasons that have nothing to do with the model.
      setNoProvider(false)
      return true
    }
  }, [])
  // ON EVERY MOUNT, not just the hero one. This used to be `if (hero)`, on the
  // theory that only a first-run user could be missing a provider - but the board
  // column's composer is the same form for the same person, and someone who never
  // connected an engine (or signed out of the one they had) reaches it far more
  // often than they reach the empty state. There, `noProvider` stayed false, so
  // `draftOrConnect` went straight to the call, the call failed, and the user got
  // "Couldn't draft that - write it below." with no hint that the cause was fixable
  // and no way to fix it. That is the exact dead end this probe exists to prevent;
  // gating it on `hero` meant it only prevented it on one of the two surfaces.
  useEffect(() => { void probeProviders() }, [probeProviders])

  /** Draft, or explain why it cannot run yet.
   *
   *  Re-probes before giving up: they may have connected one in Settings since
   *  this composer mounted, and telling a user to set up something they just set
   *  up is worse than the original dead end.
   *
   *  Shows [`AiEngineNotice`] rather than jumping straight to Settings — the jump
   *  is then the user's own click, on a button that says what it does.
   *
   *  PACED, not instant. The probe answers in single-digit milliseconds, so
   *  without the floor below the notice materialises in the same frame as the
   *  click — which reads as the form glitching rather than as an answer to what
   *  was just pressed. The button holds a "Checking…" state for long enough to
   *  be read, and the notice then fades in over its own ~600ms. */
  const [showAiNotice, setShowAiNotice] = useState(false)
  const [checking, setChecking] = useState(false)
  const draftOrConnect = useCallback(async () => {
    // Any fresh attempt retires the last answer. Without this the notice outlived
    // the problem: connect a provider in Settings by some other route, come back,
    // draft successfully, and the "Meridian needs an AI engine" card was still
    // sitting there over a draft that had plainly just worked.
    setShowAiNotice(false)
    if (!noProvider) { draftFromNote(); return }
    setChecking(true)
    const [ok] = await Promise.all([probeProviders(), wait(CHECK_MIN_MS)])
    setChecking(false)
    if (ok) { draftFromNote(); return }
    setShowAiNotice(true)
  }, [noProvider, probeProviders])

  /** A draft came back failed - work out whether that is fixable and say so.
   *
   *  The pre-flight above catches the common case (no provider configured at all),
   *  but it cannot catch the ones that only show up when the call is actually made:
   *  a CLI that is installed but signed out, an expired token, a provider that has
   *  started refusing. Those arrive here as a red line of text, and until now that
   *  was the end of the road - the message named a symptom the user had no way to
   *  act on.
   *
   *  So: re-ask health on every draft failure. If the engine is genuinely down, the
   *  answer is the same one the pre-flight gives - the notice, with its route to the
   *  provider picker - rather than an error. If health says the engine is FINE, the
   *  failure was transient (a timeout, a rate limit, a bad answer) and the honest
   *  response is the message plus a Retry, not an invitation to reconfigure
   *  something that is not broken.
   *
   *  Only ever reacts to a DRAFT failure. A create that fails on tracker
   *  permissions has nothing to do with the model, and offering to connect an AI
   *  provider there is a non-sequitur that sends the user to the wrong screen. */
  useEffect(() => {
    if (s.errorSource !== 'draft' || s.phase !== 'idle') return
    // The call that just failed said the ENGINE failed. Believe it over health, and
    // don't spend a round trip asking: health scores the last RECORDED test, so a
    // provider that connected a minute ago reads `ok` while every real call fails -
    // which left this branch showing a Try again, on a button that could only fail the
    // same way, with no route to the picker. A live failure is better evidence than a
    // remembered success.
    if (s.providerDown) { setShowAiNotice(true); setNoProvider(true); return }
    let alive = true
    void probeProviders().then(ok => { if (alive && !ok) setShowAiNotice(true) })
    return () => { alive = false }
  }, [s.error, s.errorSource, s.phase, s.providerDown, probeProviders])

  const drafting = s.phase === 'drafting'
  const creating = s.phase === 'creating'
  const busy = drafting || checking
  const ready = canCreate(s)
  const words = titleWords(s.title)
  // Null exactly when no tracker is connected - the whole "Where" block is then moot.
  const boardProvider = boardProviderFor(s, trackers.map(t => t.id))
  const onBoard = isBoardTarget(s)

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
            {/* One short line. The old subtitle spent two sentences explaining
                the form's two paths before the user had used either — the note
                box teaches the first by being there, and the fields below teach
                the second. */}
            <p className="mt-body-sm mt-1.5" style={{ color: 'var(--t-faint)' }}>
              One line is enough - Meridian turns it into a task.
            </p>
          </div>
        )}

        {/* ── The note → AI draft ─────────────────────────────────────────── */}
        {/* "In your own words" rather than "Your note" — the label's job is to
            say how to write, not to name the box. "Note" reads as a field to
            fill in correctly, which is the opposite of what this one wants. */}
        <FieldLabel>{hero ? 'In your own words' : 'What are you working on?'}</FieldLabel>
        {/* data-tour markers: inert hooks the first-run walkthrough points at.
            Attributes rather than exported refs or a `tour` prop on purpose —
            they add no behaviour, no branch and no import here, so this form
            stays a form. See ui/components/tutorial/script.ts. */}
        <textarea
          data-tour="task-note"
          ref={noteRef} rows={2} value={s.note} onChange={e => setNote(e.target.value)}
          disabled={drafting}
          placeholder="fix the flaky login test…"
          className={`${INPUT} ${FOCUS}`}
          style={{ ...inputStyle, resize: 'vertical', opacity: drafting ? 0.6 : 1 }}
        />
        <div className="flex justify-end mt-2">
          <button data-tour="task-draft" onClick={draftOrConnect} disabled={busy || !s.note.trim()}
            title={noProvider ? 'Connect an AI provider to draft this for you' : undefined}
            className={`mt-body-sm inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg ${FOCUS}`}
            style={{
              fontWeight: 700, color: '#fff', background: 'var(--btn-primary-bg)',
              opacity: busy || !s.note.trim() ? 0.5 : 1,
              cursor: busy || !s.note.trim() ? 'default' : 'pointer',
              boxShadow: '0 8px 22px -10px var(--t-accent)',
            }}>
            {/* "Draft with AI", not "Draft it" - the old label never said WHO
                was drafting, so the one button on this form that spends a model
                call read like a formatting helper. */}
            <span aria-hidden>✨</span> {drafting ? 'Drafting…' : checking ? 'Checking your AI…' : 'Draft with AI'}
          </button>
        </div>

        {/* `reason` only when the ENGINE reported the failure. The pre-flight path
            (nothing configured at all) has no reason to show - no call was made - and
            the notice's own generic headline is the right words there. */}
        {showAiNotice && (
          <AiEngineNotice
            reason={s.providerDown ? s.error ?? undefined : undefined}
            onConnect={() => {
              setShowAiNotice(false)
              window.dispatchEvent(new Event('meridian:connect-ai'))
            }}
          />
        )}

        {drafting && (
          <div className="mt-3">
            <GeneratingBar
              hue="var(--t-accent)"
              label="Drafting your task…"
              detail="Shaping your note into a title and description - they land in the fields below, yours to edit."
              note="this usually takes a few seconds"
            />
          </div>
        )}

        {/* SUPPRESSED WHILE THE NOTICE IS UP. When the cause is "there is no engine",
            the notice above already says so and carries the fix; printing the raw
            failure underneath it would restate the symptom in worse words and put a
            dead end directly below the way out. Every other error still shows -
            including a draft failure on a healthy provider, which is a real thing
            the user should see rather than a configuration problem. */}
        {s.error && !showAiNotice && (
          <div className="mt-2 rounded-lg px-3 py-2 flex items-start gap-3"
            style={{ color: 'var(--color-state-pending)', background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)' }}>
            <p className="mt-body-sm flex-1 min-w-0">{s.error}</p>
            {/* Only on a DRAFT failure, and only once the fields are still empty
                enough for a retry to be what the user wants. A create failure is
                retried with the Add button that is already on screen. */}
            {s.errorSource === 'draft' && s.note.trim() && !busy && (
              <button onClick={draftOrConnect}
                className={`mt-body-sm shrink-0 rounded-md px-2 py-1 ${FOCUS}`}
                style={{
                  fontWeight: 700,
                  color: 'var(--t-accent)',
                  background: 'color-mix(in srgb, var(--t-accent) 12%, transparent)',
                  border: '1px solid color-mix(in srgb, var(--t-accent) 30%, transparent)',
                }}>
                Try again
              </button>
            )}
          </div>
        )}

        {/* The rule between the two halves, with the choice written ON it. A
            bare line reads as "next section", so the fields below looked like
            more of the form to fill in after drafting - when they are in fact
            the ALTERNATIVE to it. Saying so here is the difference between a
            user who thinks the AI step is mandatory and one who knows it is not. */}
        <div className="my-4 flex items-center gap-3" aria-hidden>
          <span className="flex-1 border-t" style={{ borderColor: 'var(--t-hair)' }} />
          <span className="mt-body-sm shrink-0" style={{ color: 'var(--t-faint-2)' }}>
            or write it yourself
          </span>
          <span className="flex-1 border-t" style={{ borderColor: 'var(--t-hair)' }} />
        </div>

        {/* ── The task itself — ALWAYS editable, never gated on the draft ──── */}
        <div className={`rounded-xl ${drafting ? 'mer-gen-shimmer p-3 -m-0.5' : ''}`}
          style={drafting ? {
            border: '1px solid var(--gen-border)',
            background: 'linear-gradient(100deg, var(--gen-bg-1), var(--gen-bg-2), var(--gen-bg-1))',
            backgroundSize: '200% 100%',
          } : undefined}>
          <FieldLabel drafted={s.titleDrafted}>Title</FieldLabel>
          <input
            data-tour="task-title"
            value={s.title} onChange={e => setTitle(e.target.value)}
            placeholder="What needs doing?"
            className={`${INPUT} ${FOCUS}`} style={inputStyle}
          />

          <div className="mt-3">
            <FieldLabel drafted={s.descriptionDrafted}>Description</FieldLabel>
            <textarea
              data-tour="task-desc"
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
             ONE board option, not one per tracker: "personal vs. my team sees this" is
             the decision that matters, and which tracker it lands on is a detail of
             that choice - so it lives inside the option as a picker rather than
             fanning the row out into N cards that all say the same sentence. */}
        {boardProvider && (
          // data-tour: the walkthrough stops here and makes the user choose. It is
          // the only decision on this form with a consequence outside Meridian -
          // one option files a real ticket their team can see - so it is the one
          // thing here they should not discover by accident later.
          <div className="mt-4" data-tour="task-where">
            <FieldLabel>Where</FieldLabel>
            <div role="radiogroup" aria-label="Where to create this task"
              className="grid gap-2 grid-cols-1 sm:grid-cols-2">
              <TargetOption
                selected={!onBoard} onSelect={() => setTarget(LOCAL_PROVIDER)}
                label="Keep personal" hint="Only in Meridian. Nothing is posted to your board."
              />
              <TargetOption
                selected={onBoard} onSelect={() => setTarget(boardProvider)}
                label={trackers.length > 1 ? 'Create on your board' : `Create in ${trackers[0].name}`}
                hint={trackers.length > 1
                  ? 'Files a real ticket your team can see. Pick where:'
                  : `Files a real ${trackers[0].name} ticket your team can see.`}>
                {/* Only worth showing when there is genuinely a choice - with one
                    tracker connected the label above already names it. */}
                {trackers.length > 1 && (
                  <div role="radiogroup" aria-label="Which tracker to file in" className="flex flex-wrap gap-1.5 mt-2">
                    {trackers.map(t => {
                      const picked = onBoard && s.target === t.id
                      return (
                        <button key={t.id} role="radio" aria-checked={picked}
                          onClick={() => setTarget(t.id)}
                          className={`mt-chip inline-flex items-center gap-1.5 px-2 py-1 rounded-md transition-colors ${FOCUS}`}
                          style={{
                            border: `1px solid ${picked ? 'var(--color-state-approved)' : 'var(--t-ctrl-border)'}`,
                            background: picked ? 'color-mix(in srgb, var(--color-state-approved) 14%, transparent)' : 'var(--t-ctrl)',
                            color: picked ? 'var(--t-title)' : 'var(--t-faint)',
                            fontWeight: picked ? 700 : 500,
                          }}>
                          <ProviderIcon provider={t.id} size={12} />
                          {t.name}
                        </button>
                      )
                    })}
                  </div>
                )}
              </TargetOption>
            </div>
          </div>
        )}
      </div>

      {/* ── Commit. Green = you committed it (violet is reserved for AI). ───── */}
      <div className={hero ? 'mt-4' : 'shrink-0 pt-3 mt-3 border-t'}
        style={hero ? undefined : { borderColor: 'var(--t-hair)' }}>
        <button data-tour="task-add" onClick={() => createTask(day)} disabled={!ready}
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
 *  legible, because filing on a shared board is a decision worth reading first.
 *
 *  `children` (the tracker picker) sits OUTSIDE the button element, inside the same
 *  bordered shell: a button nested in a button is invalid HTML, and browsers recover
 *  from it by dropping one of them. */
function TargetOption({ selected, onSelect, label, hint, children }: {
  selected: boolean; onSelect: () => void; label: string; hint: string
  children?: React.ReactNode
}) {
  return (
    <div className="rounded-lg px-3 py-2.5 transition-colors"
      style={{
        border: `1px solid ${selected ? 'var(--color-state-approved)' : 'var(--t-ctrl-border)'}`,
        background: selected ? 'color-mix(in srgb, var(--color-state-approved) 8%, transparent)' : 'var(--t-ctrl)',
      }}>
      <button role="radio" aria-checked={selected} onClick={onSelect}
        className={`text-left w-full ${FOCUS}`} style={{ background: 'transparent' }}>
        <span className="mt-body-sm block" style={{ fontWeight: 700, color: 'var(--t-title)' }}>{label}</span>
        <span className="mt-body-sm block mt-0.5" style={{ color: 'var(--t-faint)', fontSize: 11.5, lineHeight: 1.4 }}>{hint}</span>
      </button>
      {children}
    </div>
  )
}
