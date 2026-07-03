//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The hour-row status indicators:
//   - `HourTakeover` — takes over the ENTIRE row (not a small badge) whenever
//     an hour is on the worklog pipeline's radar. Renders the SAME purple
//     shimmering card shape in both modes (ported from the Claude Design mock,
//     Meridian Timeline.dc.html — its `showGenerating` card is the only
//     current-hour treatment the design defines, there's no separate muted
//     state), varying only icon urgency / dot row / badge text:
//       * `queued`     — the CURRENT hour, still live. Most hours spend their
//         entire visible lifetime here: the DB only flips to `generating` for
//         the few seconds the `/worklog_hour` HTTP call is actually in flight
//         (a quiet hour skips that call entirely, going straight from
//         `pending` to `done`). Steady icon, no dot row, "● Tracking" badge.
//       * `generating` — the real live-generation window. Pulsing icon + ring,
//         staggered 3-dot "typing" row, "◷ In progress" badge.
//     Replaces the row's normal content (cards/Quiet/solo-strip) for either
//     mode — TimelineColumn renders this INSTEAD of them.
//   - `HourBadges` — the small "Paused" pill for tracking gaps, used on every
//     OTHER (non-current, non-generating) hour — blinking when paused THIS
//     instant (the toolbar/now-dot's `live-dot` pulse), static/dimmed when
//     it's a historical pause the hour merely overlapped.

'use client'

import { MeridianMark } from './Toolbar'

type TakeoverMode = 'queued' | 'generating'

/** Rotating copy per mode so it doesn't feel canned — picked deterministically
 *  from the hour so it doesn't flicker between re-renders. The `generating`
 *  copy matches the Claude Design mock (Meridian Timeline.dc.html) verbatim. */
const COPY: Record<TakeoverMode, [string, string][]> = {
  generating: [
    ['Generating this hour’s work log', 'Analyzing your captures — we’ll notify you the moment it’s ready.'],
  ],
  queued: [
    ['This hour is being tracked…', 'A worklog drafts itself here automatically at {NEXT} — we’ll notify you when it arrives.'],
    ['Meridian is watching this hour…', 'Keep working — nothing to do. We’ll let you know when the draft lands at {NEXT}.'],
    ['Logging this hour as you go…', 'It turns into a worklog the moment {NEXT} hits, and you’ll be notified when it does.'],
  ],
}

const SOLO_GENERATING_TITLE = 'Summarizing this hour'

export function HourTakeover({
  hour, mode, paused, nextHourLabel, isSolo,
}: {
  hour: number
  mode: TakeoverMode
  paused: boolean
  /** Only used by `queued` — when the worklog actually drafts. */
  nextHourLabel: string
  /** Solo (no-tracker) mode uses "Summarizing…" copy instead of "Generating…". */
  isSolo?: boolean
}) {
  const active = mode === 'generating'
  const lines = COPY[mode]
  const [lineHeadline, subTemplate] = lines[hour % lines.length]
  const headline = active && isSolo ? SOLO_GENERATING_TITLE : lineHeadline
  const sub = subTemplate.replace('{NEXT}', nextHourLabel)

  // One shared card shell — the Claude Design mock (Meridian Timeline.dc.html)
  // only ever renders this ONE purple shimmering card for the current hour
  // (there's no separate muted/amber "queued" treatment in the design), so
  // `queued` and `generating` are the same shell here too: only the icon's
  // urgency (ring + blink vs steady), the dot row, and the badge differ.
  return (
    <div
      className="relative flex items-center gap-3 mx-2 rounded-[13px] overflow-hidden mer-gen-shimmer"
      role="status"
      aria-live="polite"
      style={{
        padding: '16px 18px',
        border: '1px solid var(--gen-border)',
        background: 'linear-gradient(100deg, var(--gen-bg-1), var(--gen-bg-2), var(--gen-bg-1))',
        backgroundSize: '200% 100%',
        boxShadow: 'var(--gen-shadow)',
      }}
    >
      <span className="relative flex items-center justify-center shrink-0" style={{ width: 26, height: 26 }} aria-hidden="true">
        <span className="absolute rounded-full mer-gen-ring"
          style={{ width: 26, height: 26, background: 'var(--gen-ring)' }} />
        {/* The actual Meridian brand mark (Toolbar's MeridianMark) — no chip
            behind it here (unlike the nav pill's solid-dark bar, this card
            already has its own tinted background), so the icon just sits on
            the card directly instead of boxed in an extra dark square. */}
        <span className="relative flex items-center justify-center mer-gen-blink" style={{ width: 26, height: 26 }}>
          <MeridianMark size={22} />
        </span>
      </span>

      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="font-bold truncate" style={{ fontSize: 12.5, letterSpacing: '-.01em', color: 'var(--gen-title)' }}>
            {headline}
          </span>
          {/* Staggered "typing" dots — always on (queued or generating), so
              the current hour always reads as live/in-motion, not just while
              a real /worklog_hour call is in flight. */}
          <span className="flex items-end gap-[3px] shrink-0" aria-hidden="true">
            {[0, 1, 2].map(i => (
              <span key={i} className="mer-gen-dot inline-block rounded-full"
                style={{ width: 4, height: 4, background: 'var(--gen-dot)', animationDelay: `${i * 200}ms` }} />
            ))}
          </span>
        </div>
        <div className="truncate" style={{ fontSize: 11.5, lineHeight: 1.4, color: 'var(--gen-sub)', marginTop: 2 }}>
          {sub}
        </div>
      </div>

      <span className="shrink-0 whitespace-nowrap rounded-full"
        style={{
          fontFamily: 'var(--font-mono)',
          fontWeight: 700,
          fontSize: 9.5,
          letterSpacing: '.03em',
          color: 'var(--gen-badge-text)',
          background: 'var(--gen-badge-bg)',
          border: '1px solid var(--gen-badge-border)',
          padding: '4px 9px',
        }}>
        ◷ In progress
      </span>

      {paused && (
        <span className="mt-chip inline-flex items-center gap-1.5 px-2 py-1 rounded-full whitespace-nowrap shrink-0"
          style={{
            color: 'var(--color-state-pending)',
            background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)',
            border: '1px solid color-mix(in srgb, var(--color-state-pending) 24%, transparent)',
          }}>
          Paused
        </span>
      )}
    </div>
  )
}

export function HourBadges({ pausedNow, pausedHistoric }: { pausedNow: boolean; pausedHistoric: boolean }) {
  if (!pausedNow && !pausedHistoric) return null
  return (
    <div className="flex items-center gap-1.5 shrink-0 pt-0.5">
      <PausedBadge live={pausedNow} />
    </div>
  )
}

function PausedBadge({ live }: { live: boolean }) {
  return (
    <span className="mt-chip inline-flex items-center gap-1.5 px-2 py-1 rounded-full whitespace-nowrap"
      style={{
        color: 'var(--color-state-pending)',
        background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)',
        border: '1px solid color-mix(in srgb, var(--color-state-pending) 24%, transparent)',
        opacity: live ? 1 : 0.7,
      }}>
      <span className={`inline-block w-1.5 h-1.5 rounded-full ${live ? 'live-dot' : ''}`}
        style={{ background: 'var(--color-state-pending)' }} aria-hidden="true" />
      Paused
    </span>
  )
}
