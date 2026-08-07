//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// A scripted stand-in for `useWorklog`, so the walkthrough can run the REAL
// worklog flow - Generate, a match with its confidence, the ticket picker,
// Approve, Posted - inside the REAL detail panel, without any of it happening.
//
// WHY IT HAS TO BE FAKE, given the tour's rule that a control it points at
// should be the product's own: the live `useWorklog.generate` spawns the user's
// AI CLI against a day-task id, and `approve` files a comment on a real board.
// The walkthrough is standing on an EXAMPLE day whose task ids exist nowhere in
// the database and whose ticket keys belong to someone else's project, so the
// real hook can only produce an error toast, and the real approve could only
// produce a wrong comment on a stranger's ticket. Everything the user SEES and
// PRESSES is the shipped component; only the state behind it is scripted, and
// the beat after the post says so out loud.
//
// The match is deliberately a STRONG one (0.9) against a ticket that is on the
// example day's plan, because that is the case the product is actually built
// around: work you planned, matched to the ticket you planned it as. The weak
// case - work that matched nothing and gets a brand-new ticket drafted for it -
// is taught separately, on the daily summary (see `TutorialSummaryCard`), so the
// two outcomes are never conflated into one ambiguous demo.
//
// # Who calls this
// [`TutorialScreen`], which passes the result into `DayTaskDetailPanel`'s
// `worklog` override for the duration of the tour.
//
// # Related
// - `ui/components/timeline/useWorklog.ts` — the real hook this mirrors
// - `./sampleDay.ts` — `DRAFT_TASK_ID`, the card this draft belongs to

import { useCallback, useMemo, useRef, useState } from 'react'
import type { WorklogState } from '@/components/timeline/useWorklog'
import type { BoardTicket, DayTaskWorklogDraft } from '@/lib/api-types'

/** How confident the demo match is, as the panel renders it: "90% match".
 *  Exported so the beat that rings the row and the row itself cannot disagree. */
export const DEMO_MATCH_CONFIDENCE = 0.9

/** The ticket the demo draft matches - on the example day's plan, and the same
 *  key `sampleDay`'s activity-feed card carries. */
export const DEMO_MATCH_KEY = 'MER-475'

/** Long enough that the generating bar is a real state the user watches rather
 *  than a flicker, short enough that nobody wonders if it hung. The real call is
 *  far slower; the tour is not the place to reproduce that faithfully. */
const GENERATE_MS = 2200
const APPROVE_MS = 1100

/** Every open ticket the demo picker offers. Deliberately includes plausible
 *  WRONG answers - the point of the picker beat is that the match is a
 *  suggestion, and a list containing only the right answer teaches nothing. */
export function demoBoardTickets(provider: string): BoardTicket[] {
  const t = (task_key: string, title: string, issue_type: string, epic_title: string): BoardTicket =>
    ({ task_key, provider, title, issue_type, epic_title })
  return [
    t(DEMO_MATCH_KEY, 'Add cursor pagination to activity-feed API', 'Story', 'Feed performance'),
    t('MER-482', 'Fix token-refresh race condition in auth', 'Bug', 'Auth hardening'),
    t('MER-495', 'Redis session store - eviction and TTL', 'Task', 'Auth hardening'),
    t('MER-501', 'Sprint planning and board grooming', 'Task', 'Team rituals'),
    t('MER-510', 'Polish onboarding empty states', 'Task', 'Onboarding'),
  ]
}

/** The update the demo draft carries. Written in the shape the real model
 *  produces (a lead paragraph plus labelled bullet groups it chose itself), so
 *  the panel's own rendering has something honest to render. */
function demoUpdate() {
  return {
    summary:
      'Activity feed now loads instantly regardless of history size. Replaced the'
      + ' offset query with cursor pagination and shipped it after checking it on'
      + ' light, medium and very active accounts.',
    sections: [
      {
        heading: 'Decisions',
        points: [
          'Moved the feed off offset pagination - the cost grew with history length.',
          'Cursor keyed on (created_at, id) so ordering stays stable across pages.',
        ],
      },
      {
        heading: 'Verification',
        points: ['Checked against light, medium and very active accounts before shipping.'],
      },
    ],
    status: 'Shipped',
  }
}

/** Build the draft in its matched state, optionally retargeted at a ticket the
 *  user picked themselves (which is what `manual` records - the panel then shows
 *  it as THEIR choice and drops the percentage, since a confidence on a human
 *  decision is meaningless). */
function demoDraft(provider: string, taskKey: string, manual: boolean, posted: boolean): DayTaskWorklogDraft {
  const tickets = demoBoardTickets(provider)
  return {
    state: posted ? 'posted' : 'drafted',
    provider,
    targets: [{
      task_key: taskKey,
      provider,
      confidence: manual ? 0 : DEMO_MATCH_CONFIDENCE,
      manual,
      task_title: tickets.find(t => t.task_key === taskKey)?.title ?? null,
      posted,
      posted_comment_id: null,
      browse_url: null,
      outcome_unknown: false,
      error: null,
    }],
    propose: null,
    update: demoUpdate(),
    reasoning: '',
    created_task_key: null,
    error: null,
    // A fixed wall-clock stamp would read as stale ("Generated at 9:14 AM" on a
    // draft the user just watched appear), so it is stamped when the demo
    // generate resolves - see `generate` below.
    updated_at: new Date().toISOString(),
    // The walkthrough's day is a finished example - its drafts are written
    // from work that has already stopped, so nothing can have gone stale
    // behind them.
    stale_minutes: 0,
    stale: false,
  }
}

/** The walkthrough's worklog state machine. Same surface as `useWorklog`, so it
 *  drops into `DayTaskDetailPanel` unchanged; every transition is a timer. */
export function useDemoWorklog(provider: string): WorklogState {
  const [draft, setDraft] = useState<DayTaskWorklogDraft | null>(null)
  const [phase, setPhase] = useState<'loading' | 'idle' | 'generating' | 'approving'>('idle')
  const [confirming, setConfirming] = useState(false)
  // Timers are cleared on the next transition rather than on unmount: the panel
  // unmounts whenever the user clicks away from the card, and a generate that
  // silently cancelled itself there would leave the tour waiting on a draft that
  // is never coming.
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const after = useCallback((ms: number, fn: () => void) => {
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(fn, ms)
  }, [])

  const generate = useCallback(() => {
    setConfirming(false)
    setPhase('generating')
    after(GENERATE_MS, () => {
      setDraft(demoDraft(provider, DEMO_MATCH_KEY, false, false))
      setPhase('idle')
    })
  }, [after, provider])

  const approve = useCallback(() => {
    setConfirming(false)
    setPhase('approving')
    after(APPROVE_MS, () => {
      setDraft(d => (d ? { ...d, state: 'posted', targets: d.targets.map(t => ({ ...t, posted: true })) } : d))
      setPhase('idle')
    })
  }, [after])

  const retarget = useCallback((taskKey: string) => {
    setConfirming(false)
    setDraft(demoDraft(provider, taskKey, true, false))
  }, [provider])

  return useMemo<WorklogState>(() => ({
    draft,
    phase,
    error: null,
    posted: draft?.state === 'posted',
    confirming,
    setConfirming,
    generate,
    approve,
    retarget,
    // Dropping the only target would leave the draft with nowhere to post, which
    // disables Approve and strands the beat that asks for it. Nothing in the tour
    // offers this control a reason to be pressed, so it is a no-op rather than a
    // trap.
    dismiss: () => {},
    setProvider: () => {},
  }), [draft, phase, confirming, generate, approve, retarget])
}
