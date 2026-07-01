//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Drafts-review modal — a thin mount of the adapted ReviewOverlay (which owns
// its own backdrop + swipe card stack). Fed the shell's already-fetched pending
// items + mutation callbacks, so it never re-fetches.

'use client'

import type { WorklogItem } from '@/lib/api-types'
import { ReviewOverlay } from './ReviewOverlay'
import type { WorklogActions } from './useTimelineData'

export function ReviewModal({ items, actions, onClose }: {
  items: WorklogItem[]
  actions: WorklogActions
  onClose: () => void
}) {
  return <ReviewOverlay items={items} actions={actions} onClose={onClose} />
}
