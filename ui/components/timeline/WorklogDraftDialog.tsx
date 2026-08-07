//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The worklog draft, in a dialog of its own.
//
// It used to live inside the task-detail panel, stacked under "When" and "What
// was done" with a pinned action bar below it. In a 388px column that put four
// unrelated things on one surface - two blocks of evidence ABOUT the work, then a
// generated document, then its controls - and the reading fell apart: "what was
// done" and the draft update say overlapping things in different voices, one
// summarising the day for the user and one addressed to a ticket, and side by
// side the user has to work out which is which before either is useful.
//
// So the panel keeps the evidence and nothing else, and the document gets a
// surface sized for a document. One job per surface, and the panel's own scroll
// no longer competes with the draft's.
//
// # Who calls this
// [`DayTaskDetailPanel`], which renders `WorklogEntry` inline and this dialog on
// top when it is opened.
//
// # Related
// - `./WorklogActions.tsx` — the footer controls, one per phase
// - `./useWorklog.ts` — the state machine behind all of it
// - `./WorklogTargets.tsx` — the matched-ticket rows and the update body

import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { GeneratingBar } from '@/components/GeneratingBar'
import { load } from '@/lib/bridge'
import type { BoardTicket, DayTaskWorklogDraft, IntegrationsResponse } from '@/lib/api-types'
import type { Tracker } from '@/lib/integrations'
import { clockLabelFromIso } from './dayTaskLayout'
import type { SettingsSection } from './settings/types'
import type { WorklogState } from './useWorklog'
import { WorklogTicketPicker } from './WorklogTicketPicker'
import { DraftDocument } from './WorklogTargets'
import {
  ConfirmPost, ConnectTrackerCta, DraftActions, GenerateCta, PostedBar, RegenerateDraft,
  type PostedLink,
} from './WorklogActions'

/** The one line the task panel keeps: what the worklog is up to, and the way in.
 *
 *  A STATUS PLUS A VERB, never a bare button. "Generate worklog" alone says
 *  nothing about whether one already exists, so a user who drafted this yesterday
 *  and came back could not tell without pressing it - and pressing it is the one
 *  action that overwrites. The row answers "is there one?" before offering to do
 *  anything about it. */
export function WorklogEntry({ wl, hue, linkedTicket, noTracker, onOpen, onConnectTracker }: {
  wl: WorklogState
  hue: string
  linkedTicket: string | null
  /** Integrations loaded and nothing connected - there is nowhere to post. */
  noTracker: boolean
  onOpen: () => void
  onConnectTracker: () => void
}) {
  const { draft, phase, posted } = wl
  const running = phase === 'generating'
  const posts = draft?.targets.filter((t) => t.posted) ?? []
  const where = posts[0]?.task_key ?? linkedTicket

  // Nowhere to post: say so here rather than opening a dialog whose every control
  // is a dead end. The dialog's own connect CTA still exists for the case where a
  // draft already exists from a tracker that has since been disconnected.
  if (noTracker && !draft) {
    return (
      <EntryRow hue={hue} tone="quiet" onClick={onConnectTracker}
        label="Connect a tracker to auto-log this"
        detail="Meridian drafts the update, matches the issue and posts it - you approve every one."
        cta="Connect" />
    )
  }

  if (running) {
    return (
      <EntryRow hue={hue} tone="busy" onClick={onOpen}
        label="Writing the update…"
        detail="Reading the work and comparing it against today's tasks. Carry on - this runs in the background."
        cta="Watch" />
    )
  }

  if (posted) {
    return (
      <EntryRow hue={hue} tone="done" onClick={onOpen}
        label={where ? `Posted to ${where}` : 'Update posted'}
        detail="Read what went out, or draft a follow-up from the work since."
        cta="View" />
    )
  }

  if (draft) {
    return (
      <EntryRow hue={hue} tone="ready" onClick={onOpen}
        label="Draft ready for review"
        detail={draft.propose
          ? 'This did not match today\'s tasks, so Meridian wrote a new ticket for it.'
          : `Matched to ${draft.targets.map((t) => t.task_key).join(', ') || 'a ticket'}. Nothing posts until you approve.`}
        cta="Review" />
    )
  }

  return (
    <EntryRow hue={hue} tone="idle" onClick={onOpen}
      label="Write the ticket update"
      detail="Meridian turns this work into a short status update and matches it to the right issue."
      cta="Open" />
  )
}

/** One row: a state dot, a headline, a supporting line, and the way in.
 *
 *  Deliberately a single control rather than a card containing a button. The
 *  whole row is the target, so there is one thing to aim at instead of a
 *  paragraph with a small button somewhere in it. */
function EntryRow({ hue, tone, label, detail, cta, onClick }: {
  hue: string
  tone: 'idle' | 'busy' | 'ready' | 'done' | 'quiet'
  label: string; detail: string; cta: string
  onClick: () => void
}) {
  const accent = tone === 'done' ? 'var(--color-state-approved)'
    : tone === 'busy' ? 'var(--color-state-pending)'
      : tone === 'quiet' ? 'var(--t-muted)'
        : hue
  return (
    <button data-tour="wl-open" onClick={onClick}
      className="dt-wl-entry w-full text-left rounded-xl p-4 flex items-center gap-3.5"
      style={{
        background: `color-mix(in srgb, ${accent} 6%, var(--t-card))`,
        border: `1px solid color-mix(in srgb, ${accent} 26%, transparent)`,
        cursor: 'pointer',
      }}>
      <span aria-hidden className={tone === 'busy' ? 'live-dot' : undefined}
        style={{
          width: 8, height: 8, borderRadius: 99, flexShrink: 0,
          background: accent, boxShadow: `0 0 0 3px color-mix(in srgb, ${accent} 16%, transparent)`,
        }} />
      <span className="flex-1 min-w-0">
        <span className="block mt-body-sm" style={{ color: 'var(--t-title)', fontWeight: 700, fontSize: 13.5 }}>
          {label}
        </span>
        <span className="block mt-1" style={{ color: 'var(--t-muted)', fontSize: 12, lineHeight: 1.5 }}>
          {detail}
        </span>
      </span>
      <span className="mt-body-sm shrink-0 rounded-lg px-3 py-1.5"
        style={{ color: accent, fontWeight: 700, fontSize: 12.5, border: `1px solid color-mix(in srgb, ${accent} 34%, transparent)` }}>
        {cta}
      </span>
    </button>
  )
}

/** The draft itself: a modal document with its controls pinned to the bottom.
 *
 *  PORTALLED TO `document.body`, and this is load-bearing rather than tidiness.
 *  It is *called* from deep inside the 388px task column, and `position: fixed`
 *  only means "relative to the window" while no ancestor has a `transform`,
 *  `filter`, `perspective`, `contain` or `will-change` — any one of those makes
 *  that ancestor the containing block instead, and the modal silently folds
 *  itself into the column. An ancestor `opacity` below 1 does something adjacent
 *  and just as invisible: it opens a stacking context, so this z-50 stops
 *  outranking anything outside the column and the timeline paints straight
 *  through the dialog.
 *
 *  Both have already happened here, from two different animations added to that
 *  column weeks apart — and neither looks like a stacking bug on screen, it just
 *  looks like the dialog broke. Rendering into `body` removes the whole class of
 *  failure rather than the two instances of it, and means nobody has to know
 *  this rule before animating a panel.
 *
 *  It sits at z-50, above the dashboard and above the walkthrough's own surface
 *  (z-30), and below the tour overlay (z-9000) so a beat can still ring the
 *  controls in here. */
export function WorklogDraftDialog({
  wl, hue, taskTitle, linkedTicket, integrations, trackers, connected,
  boardTickets, isDemo, onClose, onOpenSettings, onOpenTask,
}: {
  wl: WorklogState
  hue: string
  taskTitle: string
  linkedTicket: string | null
  integrations: IntegrationsResponse | null
  /** Tracker display names, for the CTA copy. */
  trackers: string[]
  /** The same trackers as objects, for the propose branch's board picker. */
  connected: Tracker[]
  boardTickets?: BoardTicket[]
  /** The walkthrough is driving this - suppress the live pre-flights. */
  isDemo?: boolean
  onClose: () => void
  onOpenSettings: (section?: SettingsSection) => void
  onOpenTask: (key: string, title?: string) => void
}) {
  const { draft, phase, error, posted, confirming, setConfirming, generate, approve, retarget } = wl
  const busy = phase === 'generating' || phase === 'approving'
  const done: PostedLink[] = draft?.targets.filter((t) => t.posted) ?? []
  const noTracker = !isDemo && integrations !== null && trackers.length === 0

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape' && !busy) onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose, busy])

  // ── Is there a model to do this AT ALL ───────────────────────────────────
  //
  // The same `llm_provider_ok` the dashboard banner reads, and the same pre-flight
  // `TaskComposer` runs before "Draft with AI" — reused rather than reinvented, so
  // there is one definition of "connected" in the product instead of two that can
  // disagree.
  //
  // CHECKED ON THE CLICK, and the button keeps its own name either way. Swapping
  // the control for "Connect an AI provider" was the first attempt and it was
  // wrong twice over: this surface then never offers the feature it exists for, so
  // someone who has not connected anything cannot tell what it is FOR; and the
  // swap rides on a background read, so the button silently becomes a different
  // button between one glance and the next. Ask for the thing you want, and be
  // told why it cannot happen yet.
  //
  // SKIPPED WHEN THE WORKLOG IS SCRIPTED (the first-run walkthrough): that flow
  // never reaches a model, so the user's real provider health says nothing about
  // whether its Generate button can work, and letting it swap the control out
  // would strand the tour on a beat waiting for one that is no longer rendered.
  const [checking, setChecking] = useState(false)
  const [providerDown, setProviderDown] = useState(false)

  /** Generate — or explain why it cannot run yet.
   *
   *  RE-READS health on every press rather than trusting an earlier answer: the
   *  user may have connected a provider since this opened (quite likely, since the
   *  notice below sends them to exactly that screen), and telling someone to set up
   *  a thing they just set up is worse than the original dead end. */
  const attemptGenerate = async () => {
    if (isDemo) { generate(); return }
    setChecking(true)
    try {
      const h = await load<{ llm_provider_ok?: boolean }>('/api/health', 'get_health')
      if (h?.llm_provider_ok === false) { setProviderDown(true); return }
      setProviderDown(false)
      generate()
    } catch {
      // A FAILED PROBE MEANS PROCEED. Blocking generation because a health read
      // did not come back would break drafting for reasons that have nothing to
      // do with the model - a strictly worse failure than the one prevented.
      setProviderDown(false)
      generate()
    } finally {
      setChecking(false)
    }
  }

  // The manual ticket picker, reset whenever the draft's targets change so a
  // successful pick closes it without the caller wiring that up.
  const [picking, setPicking] = useState(false)
  const targetKeys = draft?.targets.map((t) => t.task_key).join(',')
  useEffect(() => { setPicking(false) }, [targetKeys, draft?.propose?.title])

  const generatedAt = draft ? clockLabelFromIso(draft.updated_at) : null

  // The portal host, resolved after mount because `document` does not exist
  // during the static export's prerender. Costs nothing here: this only ever
  // renders in response to a click, so there is no first paint to miss.
  const [host, setHost] = useState<HTMLElement | null>(null)
  useEffect(() => { setHost(document.body) }, [])
  if (!host) return null

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center p-6 sm:p-10 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }}
      onClick={busy ? undefined : onClose}>
      <div className="w-full rounded-2xl overflow-hidden flex flex-col bg-panel"
        style={{ maxWidth: 640, maxHeight: '86%', border: '1px solid var(--t-card-border)', boxShadow: 'var(--mt-modal-shadow)' }}
        onClick={(e) => e.stopPropagation()}>

        {/* Header: what this document is, then what it is ABOUT. The task title is
            the subtitle rather than the title - the user got here from that task
            and does not need reminding what they clicked; they need telling what
            the surface is. */}
        <div className="flex items-start justify-between gap-4 px-7 pt-6 pb-5 border-b shrink-0"
          style={{ borderColor: 'var(--t-hair)' }}>
          <div className="min-w-0">
            <p className="mt-modal-title text-title">Worklog draft</p>
            <p className="mt-1 truncate" style={{ color: 'var(--t-muted)', fontSize: 12.5 }}>
              {taskTitle}
              {generatedAt && <span style={{ color: 'var(--t-faint)' }}> · written at {generatedAt}</span>}
            </p>
          </div>
          <div className="flex items-center gap-1 shrink-0">
          {/* Rewrite lives WITH the document, next to when it was written - that
              is the fact it acts on. It used to be the first of three bordered
              buttons in the action row, where it read as a third answer to
              "where does this go" rather than a question about the text. */}
          {draft && !posted && <RegenerateDraft busy={busy} onRegenerate={generate} />}
          <button onClick={onClose} aria-label="Close" data-tour="wl-close"
            className="inline-flex items-center justify-center rounded-full bg-wrap shrink-0"
            style={{ width: 30, height: 30, color: 'var(--t-muted)' }}>
            <span className="text-[17px] leading-none">×</span>
          </button>
          </div>
        </div>

        {/* Body */}
        <div className="flex-1 min-h-0 overflow-y-auto nice-scroll px-7 py-6">
          {draft ? (
            <div className="space-y-4">
              {/* ONE CARD. `DraftDocument` holds both the target and the update -
                  when the model split the work per ticket, each body renders in
                  its own row instead and the shared block is dropped. */}
              <DraftDocument draft={draft} busy={busy} trackers={connected} onOpenTask={onOpenTask}
                onDismiss={wl.dismiss} onSetProvider={wl.setProvider} />
              <Provenance draft={draft} />
            </div>
          ) : phase === 'generating' ? (
            <GeneratingBar hue={hue} label="Generating your worklog…"
              detail="Reading your work, comparing it against today's tasks and drafting the update - you can keep using Meridian while this runs." />
          ) : (
            <EmptyDraft />
          )}
        </div>

        {/* Footer: exactly one row, whichever phase we are in. */}
        <div className="shrink-0 px-7 py-5 space-y-3"
          style={{ background: 'var(--t-card)', borderTop: '1px solid var(--t-card-border)' }}>
          {error && (
            <p className="mt-body-sm rounded-lg px-3 py-2"
              style={{ color: 'var(--color-state-pending)', background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)', fontSize: 12 }}>
              {error}
            </p>
          )}
          {phase === 'generating' ? null
            : posted ? (
              <PostedBar done={done} linkedTicket={linkedTicket} provider={draft?.provider ?? ''}
                hue={hue} busy={busy} onRegenerate={generate} />
            ) : !draft ? (
              noTracker
                ? <ConnectTrackerCta hue={hue} onConnect={() => onOpenSettings('integrations')} />
                : <GenerateCta hue={hue} trackers={trackers} checking={checking}
                    disabled={busy || phase === 'loading' || checking} onGenerate={attemptGenerate}
                    blocked={providerDown} onConnectProvider={() => onOpenSettings('intelligence')} />
            ) : picking ? (
              <WorklogTicketPicker current={draft.targets[0]?.task_key ?? null} busy={busy}
                tickets={boardTickets} onPick={retarget} onCancel={() => setPicking(false)} />
            ) : confirming ? (
              <ConfirmPost draft={draft} busy={busy} approving={phase === 'approving'}
                onApprove={approve} onCancel={() => setConfirming(false)} />
            ) : (
              <DraftActions draft={draft} hue={hue} busy={busy}
                onApprove={() => setConfirming(true)} onPick={() => setPicking(true)} />
            )}
        </div>
      </div>
    </div>,
    host,
  )
}

/** Nothing drafted yet — say what pressing the button will produce, rather than
 *  leaving the document area blank above a call to action. */
function EmptyDraft() {
  return (
    <div className="text-center py-6">
      <p className="mt-body" style={{ color: 'var(--t-title)', fontWeight: 700 }}>
        No update written yet
      </p>
      <p className="mt-2 mx-auto" style={{ color: 'var(--t-muted)', fontSize: 13, lineHeight: 1.6, maxWidth: 380 }}>
        Meridian reads what you actually did in this stretch and writes it up as a
        short status update - the decisions, what you verified, and where it landed.
      </p>
    </div>
  )
}

/** One line saying what the draft was actually compared against.
 *
 *  Without this a proposal is silently ambiguous: the user cannot tell "your board
 *  has nothing like this" from "this wasn't on today's list", and those call for
 *  completely different reactions. Say which it was, and point at the fix. Stays
 *  quiet once the user has taken over the choice - they know what they picked. */
function Provenance({ draft }: { draft: DayTaskWorklogDraft }) {
  if (draft.targets.length > 0 && draft.targets.every((t) => t.manual)) return null
  // SILENT ON A PROPOSAL. The card's own header already says it in four words -
  // "new ticket, nothing on your plan matched" - and this then said the same
  // thing again in two sentences directly under it, ending with an instruction
  // to use a button that is six inches further down and named for itself.
  if (draft.propose) return null
  const text = 'Matched against today\'s tasks only, not your whole board. Remove any that don\'t fit, or pick a different ticket below.'
  return (
    <p className="mt-body-sm" style={{ color: 'var(--t-faint)', fontSize: 11.5, lineHeight: 1.5 }}>
      {text}
    </p>
  )
}
