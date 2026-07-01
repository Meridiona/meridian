//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Board-cleanup modal — a thin mount of CleanupOverlay (which owns its own
// backdrop + swipe-card queue), same convention as ReviewModal/ReviewOverlay.

'use client'

import { CleanupOverlay } from './CleanupOverlay'

export function CleanupModal({ onClose }: { onClose: () => void }) {
  return <CleanupOverlay onClose={onClose} />
}
