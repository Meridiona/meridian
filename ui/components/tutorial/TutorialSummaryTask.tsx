//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The summary's SECOND view: one strand of work, and where its update is going.
//
// Split out of `TutorialSummaryCard` when that file crossed the 500-line rule.
// The seam is the one the card already had - the home view is the DAY, this is
// ONE THING in it - and the two share nothing but the state the card threads
// through, so nothing had to be invented to separate them.
//
// THE WORKLOG HALF IS THE SHIPPED COMPONENTS, not a replica of them. It used to
// be a hand-written copy: its own "where it goes" box, its own picker, its own
// post button, its own posted pill. Two consequences, both bad. The user learned
// a screen that does not exist - every control sat somewhere else, and the
// override was a small text button in a section header while the primary action
// was a paragraph further down, so the two halves of one decision were nowhere
// near each other. And it drifted: the real draft surface was redesigned twice
// while this one kept the old shape, so the tour taught last month's product.
// `DraftTargets`, `UpdateBody`, `DraftActions`, `WorklogTicketPicker`,
// `ConfirmPost` and `PostedBar` are now the SAME modules the task panel's draft
// dialog renders; only the draft behind them is scripted, exactly as
// `demoWorklog` does for the first post.
//
// WHAT IT TEACHES, and why it is the harder of the two worklog beats. The tour
// has already driven a worklog through the real detail panel by this point:
// planned work, matched to the ticket it was planned as, approved, posted. That
// is the easy case. This is the other one - work that came up, that matches
// nothing on the plan - and how a tool answers THAT is what decides whether it
// gets trusted with the rest. Meridian's answer is that it neither shrugs nor
// files it against the nearest half-match: it writes the ticket, in full, and
// hands the user the choice of creating it or pointing the update at something
// they already have. Both branches live here, and the button's own label is what
// says which one is about to happen.
//
// WHY THE POST IS FAKE HERE and the drag in the planner was real: the drag lands
// a row in the user's own plan, which is theirs and reversible. This would file a
// comment on a real ticket on a shared board, on an example day that never
// happened, using someone else's issue keys. The beat that follows says so out
// loud rather than letting the checkmark imply a post that did not happen.
//
// # Who calls this
// [`TutorialSummaryCard`], once the user opens a row on the home view.
//
// # Related
// - `./TutorialSummaryCard.tsx` — the home view, and the state this renders from
// - `./scriptDay.ts` — the beats that ring each section by name
// - `./demoWorklog.ts` — the same demonstration in the REAL detail panel
// - `ui/components/timeline/WorklogActions.tsx` — the footer controls, shared

import { useState } from 'react'
import {
  ConfirmPost, DraftActions, PostedBar, type PostedLink,
} from '@/components/timeline/WorklogActions'
import { DraftDocument } from '@/components/timeline/WorklogTargets'
import { CopyUpdate } from '@/components/timeline/WorklogUpdateBody'
import { WorklogTicketPicker } from '@/components/timeline/WorklogTicketPicker'
import { PROPOSED_TICKET, clock, dur } from './TutorialSummaryCard'
import { demoBoardTickets } from './demoWorklog'
import type { DayTask, DayTaskWorklogDraft } from '@/lib/api-types'

/** The scripted draft the summary's worklog surfaces render from.
 *
 *  Shaped exactly as the real generator's output so the shipped components have
 *  something honest to render: a PROPOSAL while nothing on the plan matched, and
 *  a single manual TARGET once the user overrules it with the picker (`manual`,
 *  so the row says "you picked this" rather than inventing a confidence for a
 *  decision the model did not make). */
function summaryDraft(
  task: DayTask,
  providerId: string,
  mode: 'propose' | 'match',
  target: string | null,
  posted: boolean,
): DayTaskWorklogDraft {
  const tickets = demoBoardTickets(providerId)
  const update = {
    summary: task.summary.slice(0, 2).join(' '),
    sections: [],
    status: posted ? 'Fix in review' : 'Fix open for review',
  }
  const base = {
    state: (posted ? 'posted' : 'drafted') as DayTaskWorklogDraft['state'],
    provider: providerId,
    update,
    reasoning: '',
    error: null,
    updated_at: new Date().toISOString(),
    // The walkthrough's day is a finished example - its drafts are written
    // from work that has already stopped, so nothing can have gone stale
    // behind them.
    stale_minutes: 0,
    stale: false,
  }
  const at = (task_key: string, title: string | null, manual: boolean) => ({
    task_key,
    provider: providerId,
    confidence: manual ? 0 : 1,
    manual,
    task_title: title,
    posted,
    posted_comment_id: null,
    browse_url: null,
    outcome_unknown: false,
    error: null,
  })

  if (mode === 'propose') {
    return {
      ...base,
      propose: {
        issue_type: PROPOSED_TICKET.issueType,
        title: PROPOSED_TICKET.title,
        description: PROPOSED_TICKET.description,
      },
      // Once created, the ticket exists and has a key - so the posted bar can
      // name what it landed on, the way the real flow does after the create.
      created_task_key: posted ? PROPOSED_TICKET.key : null,
      targets: posted ? [at(PROPOSED_TICKET.key, PROPOSED_TICKET.title, false)] : [],
    }
  }
  const chosen = tickets.find(t => t.task_key === target)
  return {
    ...base,
    propose: null,
    created_task_key: null,
    targets: target ? [at(target, chosen?.title ?? null, true)] : [],
  }
}

export function TaskView({ task, provider, mode, target, picking, phase, onBack, onPick, onTogglePicker, onPost }: {
  task: DayTask
  provider: { id: string; name: string } | null
  /** `propose` = nothing on the plan matched, so a new ticket is drafted;
   *  `match` = the user overruled that and picked an existing one. */
  mode: 'propose' | 'match'
  target: string | null
  picking: boolean
  phase: 'idle' | 'posting' | 'posted'
  onBack: () => void
  onPick: (key: string) => void
  onTogglePicker: () => void
  onPost: () => void
}) {
  const lo = task.segments[0]?.start ?? '09:00'
  const hi = task.segments[task.segments.length - 1]?.end ?? '09:00'
  // The confirm step is local because nothing outside this view needs it - the
  // card owns what the draft IS, this owns how far through posting it the user
  // has got.
  const [confirming, setConfirming] = useState(false)
  const providerId = provider?.id ?? ''
  const posted = phase === 'posted'
  const busy = phase === 'posting'
  const draft = summaryDraft(task, providerId, mode, target, posted)
  const done: PostedLink[] = draft.targets.filter(t => t.posted)
  const hue = 'var(--color-state-proposal)'

  return (
    <>
      <button data-tour="sum-back" onClick={onBack}
        className="mt-body-sm hover:opacity-70"
        style={{ color: 'var(--t-muted)', cursor: 'pointer' }}>
        ‹ Back to summary
      </button>

      <div className="flex items-start gap-3 mt-4">
        <span className="rounded-full shrink-0 mt-1.5"
          style={{ width: 9, height: 9, background: 'var(--color-state-proposal)' }} />
        <div className="min-w-0">
          <p className="mt-label" style={{ color: 'var(--t-faint-2)' }}>
            WORKLOG UPDATE · {dur(task.minutes)} · {clock(lo)} - {clock(hi)}
          </p>
          <h2 className="mt-1.5" style={{
            font: '800 20px var(--font-sans)', letterSpacing: '-0.024em',
            lineHeight: 1.2, color: 'var(--t-title)',
          }}>
            {task.title}
          </h2>
        </div>
      </div>

      {/* ── The draft, laid out exactly as the dialog lays it out ─────────────
          It was three blocks, then two, then one card, and it is now the same
          frameless `DraftDocument` the draft dialog renders. The count kept
          coming down for the same reason each time: every frame claimed these
          were separate things to weigh, and they are one object - a ticket and
          the text that goes on it. A "WHAT YOU DID" card led the original and
          bulleted the same work the update describes in prose (the exact
          duplication that got the draft moved out of the task panel).
          The `sum-update` / `sum-where` anchors the script rings live inside
          `DraftDocument` now, on the update and the destination. */}
      <div className="mt-6">
        <DraftDocument draft={draft} busy={busy} trackers={[]}
          onOpenTask={() => {}} onDismiss={() => {}} onSetProvider={() => {}} />
      </div>

      {/* ── The controls, in the shipped arrangement ──────────────────────────
          THE OVERRIDE SITS BESIDE THE PRIMARY. It used to be a small text button
          up in the section header, so "file this somewhere else" and "create it
          and post" - the two halves of one decision - were a whole screen apart,
          and the user met the choice before they had read what they were
          choosing about. They are one row now, and it is the same row the draft
          dialog shows: quiet things you can do TO the draft above, the decision
          about where it goes below. */}
      {!posted && !picking && !confirming && (
        <div className="mt-5 -ml-1.5">
          <CopyUpdate update={draft.update} />
        </div>
      )}
      <div className="mt-3">
        {posted ? (
          <PostedBar done={done} linkedTicket={null} provider={providerId}
            hue={hue} busy={busy}
            // Nothing in the tour offers a reason to press this, and the beat
            // that follows closes the summary - so it is inert rather than a
            // control that pretends to redraft an example day.
            onRegenerate={() => {}} />
        ) : picking ? (
          <WorklogTicketPicker current={target} busy={busy} tickets={demoBoardTickets(providerId)}
            onPick={onPick} onCancel={onTogglePicker} />
        ) : confirming ? (
          <ConfirmPost draft={draft} busy={busy} approving={busy}
            onApprove={onPost} onCancel={() => setConfirming(false)} />
        ) : (
          // Two controls, one decision - where this goes. Rewriting lives on the
          // document itself now (`RegenerateDraft`, in the draft dialog's
          // header), which the summary does not render.
          <DraftActions draft={draft} hue={hue} busy={busy}
            onApprove={() => setConfirming(true)} onPick={onTogglePicker} />
        )}
      </div>
    </>
  )
}
