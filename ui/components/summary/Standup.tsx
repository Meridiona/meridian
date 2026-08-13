//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The day, as the three or four lines you would actually say tomorrow morning.
//
// THE ONE THING ON THIS SCREEN WRITTEN FOR SOMEBODY ELSE. Everything else in the
// summary is addressed to the person who did the work - how the day went, what it
// was like. This is addressed to their team, and its whole value is that it can be
// pasted into Slack without editing. That is why it gets a Copy button and no other
// block does, and why it sits beside the plan rather than under the prose: it is an
// OUTPUT of the day, the same kind of thing as a worklog draft.
//
// COPIES AS PLAIN LINES, not as the bullets on screen. The `•` glyphs here are the
// list's own drawing - Slack and every ticket field render their own - so pasting
// them would produce a double bullet on every line. The model is told not to include
// them either (`assets/prompts/daily-summary.md`) and any it adds anyway are stripped
// server-side in `day_summary::generate::clean_standup_line`.
//
// It renders NOTHING when there are no lines, which covers the fallback path (the
// LLM call failed, so the deterministic half of the screen is all that is true) and
// every summary composed before migration 080. An empty bordered box captioned
// "standup" would be a promise the screen could not keep.
//
// # Who calls this
// [`DaySummaryOverlay`], in the right column of the plan row.
//
// # Related
// - `ui/components/tutorial/TutorialSummaryCard.tsx` — the walkthrough's scripted
//   version of this same block, which is where the shape came from.
// - `src/day_summary/generate.rs` — where the lines are parsed and cleaned.

'use client'

import { useEffect, useState } from 'react'

/** How long the button stays in its confirmed state. Long enough to be seen after
 *  the eye has moved to check the copy landed; short enough that the button is back
 *  to offering the action before anyone wants it again. */
const COPIED_MS = 2200

export function Standup({ lines }: { lines: string[] }) {
  const [copied, setCopied] = useState(false)

  // Cleared on unmount as well as on the timer: the summary can be closed (or the
  // day paged) mid-confirmation, and a timer writing state into an unmounted card
  // is a warning in the console for no benefit.
  useEffect(() => {
    if (!copied) return
    const t = setTimeout(() => setCopied(false), COPIED_MS)
    return () => clearTimeout(t)
  }, [copied])

  if (lines.length === 0) return null

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(lines.join('\n'))
      setCopied(true)
    } catch {
      // A denied clipboard is not worth an error state on a convenience button -
      // the lines are on screen and can be selected by hand.
    }
  }

  return (
    <div
      className="rounded-xl p-3.5 h-full"
      style={{ background: 'var(--t-box)', border: '1px solid var(--t-card-border)' }}
    >
      <div className="flex items-center justify-between gap-2 mb-2.5">
        <p className="mt-label" style={{ color: 'var(--t-faint-2)' }}>STANDUP - READY TO PASTE</p>
        <button
          onClick={copy}
          className="rounded-md px-2 py-1 shrink-0 hover:opacity-80"
          style={{
            fontSize: 11,
            fontWeight: 700,
            cursor: 'pointer',
            color: 'var(--accent)',
            background: 'color-mix(in srgb, var(--accent) 12%, transparent)',
            border: '1px solid color-mix(in srgb, var(--accent) 30%, transparent)',
          }}
        >
          {copied ? 'Copied ✓' : 'Copy'}
        </button>
      </div>
      <div className="flex flex-col gap-1.5">
        {lines.map((l, i) => (
          <p
            key={i}
            className="mt-body-sm"
            style={{ color: 'var(--t-muted)', lineHeight: 1.5 }}
          >
            • {l}
          </p>
        ))}
      </div>
    </div>
  )
}
