//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The walkthrough's title sequence: the app mark, the name, then two lines that
// arrive a word at a time. Plays once, on its own, before the first beat.
//
// Why it advances itself, when every other narration beat waits on Next: the
// opening is the one moment with nothing on screen to point at yet, so there is
// nothing for a slower reader to lose. A card asking the user to press "Let's
// go" before anything has happened also makes the product negotiate for
// permission to do the thing they just asked for, and puts a click between them
// and a voiceover that starts when the tour does. From beat one onward, advance
// is back under the user's thumb, where reading speed actually matters.
//
// The word-at-a-time reveal is what sets the pace. A line that appears all at
// once is read in a second and then sits there; revealed word by word it is read
// at the speed it is written, which is also roughly the speed it is spoken - so
// the voiceover and the screen stay together without either being synced to the
// other. Every duration below is a named constant and the total is DERIVED from
// them (see `totalMs`), because the failure mode of hand-tuning two numbers that
// have to agree is a card that dissolves mid-word.
//
// Deliberately plain otherwise. An earlier version had drifting gradient washes,
// a progress row and a "click to skip" hint - a lot of motion in front of
// someone who has not seen the product yet, and none of it said anything. Type
// is the app's own (`--font-sans` at the brand's weight/tracking, matching
// `app/setup/atoms.tsx`), not a display face this screen invented for itself.
//
// # Who calls this
// [`useTutorial`], implementing `Stage.intro`; the copy comes from `script.ts`.
//
// # Related
// - `./script.ts` — `INTRO_LINES`, the lines this reveals
// - `./TutorialOverlay.tsx` — the narration layer that takes over afterwards

import { useEffect, useMemo, useRef, useState } from 'react'

/** Mark + wordmark entrance, before the first word appears. */
const MARK_MS = 950
/** Gap between one word landing and the next starting. This is the reading
 *  pace: slower than a fast reader would choose, which is the point — the line
 *  should still be arriving when a first-time user finishes the last word. */
const WORD_STEP_MS = 165
/** How long one word takes to rise into place. Overlaps the next word's step,
 *  so the line flows rather than ticking. */
const WORD_IN_MS = 560
/** Beat between the two lines. Long enough to read as a new sentence. */
const LINE_GAP_MS = 700
/** Hold on the finished card before it dissolves. */
const HOLD_MS = 2200
const FADE_MS = 700

export function TutorialIntro({ lines, onDone }: {
  lines: string[]
  onDone: () => void
}) {
  const [out, setOut] = useState(false)

  // `onDone` settles a promise held by the running script, so a changed
  // identity must never restart the sequence or fire it twice.
  const doneRef = useRef(onDone)
  doneRef.current = onDone

  // Per-word start offsets, and the point at which the last word has landed.
  // Computed once so the CSS delays and the dissolve timer cannot disagree.
  const { starts, totalMs } = useMemo(() => {
    const out: number[][] = []
    let t = MARK_MS
    for (const line of lines) {
      const words = line.split(/\s+/).filter(Boolean)
      out.push(words.map((_, i) => t + i * WORD_STEP_MS))
      t += words.length * WORD_STEP_MS + LINE_GAP_MS
    }
    const last = out.at(-1)?.at(-1) ?? MARK_MS
    return { starts: out, totalMs: last + WORD_IN_MS + HOLD_MS }
  }, [lines])

  useEffect(() => {
    const t = setTimeout(() => setOut(true), totalMs)
    return () => clearTimeout(t)
  }, [totalMs])

  useEffect(() => {
    if (!out) return
    const t = setTimeout(() => doneRef.current(), FADE_MS)
    return () => clearTimeout(t)
  }, [out])

  return (
    <div
      // Silent click-through. No label for it: naming an escape hatch on the
      // opening card invites the user to treat the tour itself as something to
      // get out of, and the overlay's Skip control is visible from the very next
      // frame anyway.
      onClick={() => setOut(true)}
      className="fixed inset-0 flex flex-col items-center justify-center"
      style={{
        zIndex: 9500,
        cursor: 'default',
        background: 'var(--win-bg)',
        opacity: out ? 0 : 1,
        transition: `opacity ${FADE_MS}ms ease`,
      }}
    >
      <div className="flex flex-col items-center text-center px-10" style={{ maxWidth: 520 }}>
        <div className="flex flex-col items-center" style={{
          animation: `mer-intro-rise .9s cubic-bezier(.2,.8,.25,1) both`,
        }}>
          <Mark size={64} />
          <p style={{
            marginTop: 18,
            font: '750 33px var(--font-sans)',
            letterSpacing: '-.03em',
            color: 'var(--t-title)',
          }}>Meridian</p>
        </div>

        {/* Fixed min-heights so the block does not grow under the wordmark as
            each line arrives — the words appear, the layout does not shift. */}
        <Line words={lines[0]} starts={starts[0]} lead />
        <Line words={lines[1]} starts={starts[1]} />
      </div>

      <style>{`
        @keyframes mer-intro-rise {
          from { opacity: 0; transform: translateY(12px) }
          to   { opacity: 1; transform: none }
        }
        @keyframes mer-intro-word {
          from { opacity: 0; transform: translateY(9px); filter: blur(3px) }
          to   { opacity: 1; transform: none;            filter: blur(0px) }
        }
      `}</style>
    </div>
  )
}

/** One line, revealed a word at a time off the precomputed `starts` offsets.
 *  `lead` is the first line — a shade larger and in the title colour, so the two
 *  lines read as a statement and its footnote rather than as one long block. */
function Line({ words, starts, lead }: { words?: string; starts?: number[]; lead?: boolean }) {
  if (!words || !starts) return null
  return (
    <p style={{
      marginTop: lead ? 22 : 9,
      minHeight: lead ? 26 : 23,
      font: lead ? '600 19px/1.4 var(--font-sans)' : '500 15px/1.5 var(--font-sans)',
      letterSpacing: lead ? '-.01em' : undefined,
      color: lead ? 'var(--t-title)' : 'var(--t-muted)',
      textWrap: 'pretty',
    }}>
      {words.split(/\s+/).filter(Boolean).map((w, i) => (
        <span key={i} style={{
          display: 'inline-block',
          marginRight: '0.26em',
          // `backwards`, NOT `both`. The resting style below IS the animation's
          // end state, so filling forwards buys nothing - and it costs the word
          // a `filter` that stays applied for the rest of the card's life. A
          // filtered element is its own composited layer in WebKit, rasterised
          // once and then reused; the moment something makes the card repaint
          // (the hold ending, the opacity dissolve, a window resize) the cached
          // raster is scaled back up and the word goes soft. It reads as the
          // last word blurring "after a while", because the last word is the one
          // still on screen when the hold and the fade arrive. Filling backwards
          // covers the delay with the blurred `from` and then releases the
          // element entirely, so a landed word is plain text again.
          animation: `mer-intro-word ${WORD_IN_MS}ms cubic-bezier(.2,.8,.25,1) ${starts[i]}ms backwards`,
        }}>{w}</span>
      ))}
    </p>
  )
}

/** The real app icon. Same crop as `app/setup/atoms.tsx`'s `Logo` - the mark's
 *  bounding box sits above the PNG canvas's geometric centre, so it needs a
 *  146% zoom to stay whole under `backgroundPosition: center`. */
function Mark({ size }: { size: number }) {
  return (
    <span aria-hidden="true" className="shrink-0" style={{
      width: size, height: size, borderRadius: size / 3.6,
      backgroundImage: 'url(/meridian-logo.png)',
      backgroundSize: '146% 146%', backgroundPosition: 'center', backgroundRepeat: 'no-repeat',
    }} />
  )
}
