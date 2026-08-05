//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// What the right panel says while `Stage.replayDay` rebuilds the example day.
//
// It replaced a clock. The clock was doing the job the TIMELINE should have been
// doing: a dial ticking 9 AM to 6 PM beside a column that filled silently made
// the passage of time a thing happening over HERE, in a decoration, while the
// actual product surface looked like it was being filled by something offstage.
// Moving the clock onto the rail itself - a descending now-line, hour dots
// lighting as it passes them (`DayTaskColumn`'s `replayNowMin`) - puts the time
// and the work on the same axis, which is where they are in the real product.
// That is the marketing demo's own build phase (`advanceNow` in demo.js), and it
// is why the ring, the hands and the AM/PM label are gone rather than restyled.
//
// So this panel stops competing and does the one thing the left side cannot say
// for itself: that this is not a rendering of a finished day but the thing
// Meridian does every hour, unattended, and that the user finds out without
// looking. Deliberately still - the movement belongs on the rail.
//
// # Who calls this
// [`TutorialScreen`], in place of the right panel for the replay's duration.
//
// # Related
// - `./engine.ts` — `Stage.replayDay`, which drives the minute
// - `ui/components/timeline/DayTaskColumn.tsx` — `replayNowMin`, the lit rail

/** The hour hand's job, now that there is no hour hand: a plain readout of where
 *  the replay has got to, for anyone who missed the label on the rail. */
function clockText(minute: number): string {
  const abs = 540 + Math.max(0, Math.min(540, minute))
  const h = Math.floor(abs / 60)
  const m = Math.floor(abs % 60)
  const h12 = h % 12 === 0 ? 12 : h % 12
  return `${h12}:${String(m).padStart(2, '0')} ${h >= 12 ? 'PM' : 'AM'}`
}

export function ReplayAside({ minute, note = null }: {
  minute: number
  /** The current narration line from `Stage.replayDay`'s marks, or null. */
  note?: string | null
}) {
  const hoursDone = Math.max(0, Math.floor(minute / 60))
  return (
    <div className="h-full flex flex-col items-center justify-center gap-5 px-9 text-center">
      <div className="flex items-center gap-2.5">
        <span className="inline-block rounded-full mer-pulse"
          style={{ width: 8, height: 8, background: 'var(--accent)' }} />
        <p className="mt-label" style={{ color: 'var(--accent)' }}>Rebuilding the day</p>
      </div>

      <p style={{ font: '800 30px var(--font-sans)', letterSpacing: '-.02em', color: 'var(--t-title)' }}>
        {clockText(minute)}
      </p>

      {/* ONE LINE, and it is the script's. This panel used to carry two standing
          paragraphs AND the narration ran in the overlay bar over the timeline:
          three blocks of prose competing for someone whose eyes are supposed to
          be on a column filling itself. Nobody reads that much while watching
          something move, and the overlay bar physically covered the cards it was
          describing. So the narration moved here - the replay's own column, with
          nothing else in it - and everything it was competing with is gone.

          The count is a fallback for the gaps between marks, and it is a count
          rather than a progress bar on purpose: a bar promises a job that ends,
          and the hourly fold runs for as long as the user works. */}
      <p className="mt-body" style={{
        color: 'var(--t-title)', lineHeight: 1.55, maxWidth: 270, textWrap: 'pretty',
        minHeight: 72,
      }}>
        {note ?? (hoursDone === 0
          ? 'Meridian folds each hour into a task as it finishes.'
          : `${hoursDone} hour${hoursDone === 1 ? '' : 's'} written up so far.`)}
      </p>

      <style>{`
        .mer-pulse { animation: mer-pulse 1.6s ease-in-out infinite }
        @keyframes mer-pulse {
          0%, 100% { opacity: 1; transform: scale(1) }
          50%      { opacity: .45; transform: scale(.82) }
        }
      `}</style>
    </div>
  )
}
