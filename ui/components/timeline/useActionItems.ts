//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Assembles the standardised "action stack" shown at the top of the Overview
// panel (connected mode). One priority-ordered ActionItem[], each rendered by
// <ActionCard>. Replaces the three ad-hoc surfaces that each hand-rolled their
// own layout + count: the drafts CTA, the board-cleanup CTA, and the standalone
// top-of-app MustFixBanner.
//
// The two board-hygiene tiers are split DISJOINTLY off the SAME hygiene data —
// must-fix (tickets missing must-have info) vs. the remaining cleanup-worthy
// tickets — so a ticket lands in exactly one tier and the two counts can never
// double-count or disagree. Their sum equals the Cleanup modal's queue length.

import { useMemo } from 'react'
import { filterByConnectedProviders } from '@/lib/integrations'
import { hasMustFix, isCleanupWorthy } from '@/lib/hygiene'
import { isPending } from './types'
import type { TimelineData } from './useTimelineData'
import type { ActiveModal } from './MeridianTimelineShell'

export type ActionKind = 'mustfix' | 'cleanup' | 'drafts'

/** Whether `kind`'s card is currently dismissed. `snoozes` is the
 *  `get_action_card_snoozes` map — it only ever contains still-active
 *  snoozes (the reader filters out expired ones), so presence alone decides;
 *  no date comparison belongs on this side. Exported standalone (rather than
 *  inlined in the hook) so it's unit-testable without a React render harness
 *  (this repo has none — see __tests__/oauth-setup-lifecycle.test.ts). */
export function isSnoozed(kind: ActionKind, snoozes: Record<string, string>): boolean {
  return Boolean(snoozes[kind])
}

/** One entry in the action stack. Purely presentational data — no counts leak
 *  in from elsewhere; everything the card renders is on this object. */
export interface ActionItem {
  kind: ActionKind
  count: number
  icon: string
  title: string
  subtitle: string
  cta: string
  /** Which modal `onOpen` should open when the card body is clicked. */
  modal: ActiveModal
}

/** Build the action stack from live working-set state. Order is urgency:
 *  must-fix (blocks accurate tracking) → cleanup (nice-to-have) → drafts
 *  (routine review). Change the push order below to reorder the stack app-wide.
 *  Solo users (no connected tracker) get an empty stack. */
export function useActionItems(data: TimelineData): ActionItem[] {
  const { tasks, integrations, items, isSolo, actionCardSnoozes } = data
  return useMemo(() => {
    if (isSolo) return []
    const out: ActionItem[] = []

    const hygienic = filterByConnectedProviders(tasks, integrations).filter(t => t.hygiene)
    const mustFix = hygienic.filter(t => hasMustFix(t.hygiene!.issues)).length
    const cleanup = hygienic.filter(t => !hasMustFix(t.hygiene!.issues) && isCleanupWorthy(t.hygiene!)).length
    const drafts = items.filter(isPending).length

    if (mustFix > 0 && !isSnoozed('mustfix', actionCardSnoozes)) out.push({
      kind: 'mustfix', count: mustFix, icon: '⚠️', modal: 'cleanup',
      title: `${mustFix} ticket${mustFix === 1 ? '' : 's'} need must-have info`,
      subtitle: 'Missing a due date, description, or clear title',
      cta: 'Fix',
    })
    if (cleanup > 0 && !isSnoozed('cleanup', actionCardSnoozes)) out.push({
      kind: 'cleanup', count: cleanup, icon: '🧹', modal: 'cleanup',
      title: 'Board cleanup available',
      subtitle: `${cleanup} issue${cleanup === 1 ? '' : 's'} make matching harder`,
      cta: 'Review',
    })
    if (drafts > 0 && !isSnoozed('drafts', actionCardSnoozes)) out.push({
      kind: 'drafts', count: drafts, icon: '📝', modal: 'review',
      title: `${drafts} draft${drafts === 1 ? '' : 's'} to review`,
      subtitle: 'Approve, dismiss or edit',
      cta: 'Review',
    })
    return out
  }, [tasks, integrations, items, isSolo, actionCardSnoozes])
}
