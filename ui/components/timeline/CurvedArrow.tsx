//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Small decorative curved connector used above the "Connect a tracker" CTAs
// (OverviewPanel, HourDetailPanel) — curves down from the content above (the
// Activity summary / today's stats) into the CTA below it, with a flowing
// animated dash and a gentle float, so the CTA reads as pointed-to rather
// than a disconnected banner. Replaces a flat "→" character.

'use client'

export function CurvedArrow({ size = 40 }: { size?: number }) {
  return (
    <svg width={size} height={size * 0.68} viewBox="0 0 44 30" fill="none" className="mer-curved-arrow" aria-hidden="true">
      <path d="M4 2 C4 16, 16 24, 35 26" stroke="var(--color-state-proposal)" strokeWidth="2.25"
        strokeLinecap="round" strokeDasharray="3 6" className="mer-arrow-flow" />
      <path d="M27 21 L36 27 L28 29" stroke="var(--color-state-proposal)" strokeWidth="2.25"
        strokeLinecap="round" strokeLinejoin="round" fill="none" />
    </svg>
  )
}
