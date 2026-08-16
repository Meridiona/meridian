//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The walkthrough's story beats: the whole window, one line, typed.
//
// Between the hands-on stretches the tour stops instructing and says what all
// of it is FOR. That cannot be done from a bubble parked beside a button -
// there is nothing to point at, and the same surface that has been saying
// "press this" cannot also say "here is why you would want to". The change of
// surface IS the signal: ring and caption means do something, this means listen.
//
// Ported from a Claude Design mockup (`Onboarding Tour.dc.html`), with two
// deliberate departures. Its heroes are Instrument Serif; that face was retired
// from the app (globals.css: "There is deliberately no --font-serif", and
// layout.tsx records the setup and uninstall wizards dropping it), so these are
// the app's own display treatment at the mockup's scale - the register comes
// from size, colour and space rather than from a second typeface. And its
// colours are literal hex; every one of them turned out to be a `--t-*` token
// value, so they are written as the tokens and the surface themes itself.
//
// THIS ALSO REPLACES THE OLD TITLE SEQUENCE. `TutorialIntro` rendered a
// full-screen typed card too, and two components doing that is how the tour
// ended up with two narration treatments once already. The opening is simply a
// takeover with `mark` and a `hold`.
//
// # Who calls this
// [`useTutorial`], implementing `Stage.takeover`; the copy comes from
// `script.ts` and `scriptDay.ts`.
//
// # Related
// - `./useTypewriter.ts` — the reveal, shared with the anchored caption
// - `./engine.ts` — `TakeoverBeat` and `CHAPTERS`

import { useEffect, useRef } from 'react'
import { CHAPTERS, type ChapterId, type TakeoverBeat } from './engine'
// Shared rather than re-drawn: the confetti is the tour's one celebration, and
// two copies of it is how the close ends up looking subtly unlike itself.
import { Confetti } from './TutorialOverlay'
import { useTypewriter } from './useTypewriter'

/** How long the sub-line and CTA take to arrive after the hero lands. Long
 *  enough to read as a second thought rather than as part of the same block. */
const REVEAL_MS = 420

export function TutorialTakeover({ beat, chapter, onNext, onSkip, skipLabel }: {
  beat: TakeoverBeat
  /** Which chapter the tour is in, for the progress row. */
  chapter: ChapterId
  /** Advance to the next beat. Called by the CTA, or by `hold` expiring. */
  onNext: () => void
  /** Leave the tour. Rendered HERE as well as in the overlay because this
   *  surface covers the overlay - see the note on `zIndex` below. */
  onSkip: () => void
  skipLabel: string
}) {
  const { shown, done, finish } = useTypewriter(beat.line)

  // `hold` beats advance themselves once the line has landed. Started off
  // `done` rather than off a total duration, so a user who clicks to finish the
  // typing early gets their hold from the moment they actually finished
  // reading it - not from a schedule set before they intervened.
  const nextRef = useRef(onNext)
  nextRef.current = onNext
  useEffect(() => {
    if (!done || beat.hold === undefined) return
    const t = setTimeout(() => nextRef.current(), beat.hold)
    return () => clearTimeout(t)
  }, [done, beat.hold])

  const at = CHAPTERS.findIndex((c) => c.id === chapter)

  return (
    <div
      // ABOVE the tour overlay (9000), because a story beat owns the window -
      // a cursor gliding across it, or a caption bubble from the last beat, would
      // both be leftovers from the mode this surface exists to leave. The cost
      // is that the overlay's Skip is covered, which is why one is rendered here
      // too: same handler, never both on screen at once.
      style={{ position: 'fixed', inset: 0, zIndex: 9500, background: 'var(--win-bg)' }}
    >
      {/* Click anywhere to land the rest of the line. A line typed at reading
          speed is a pace, not a gate - someone re-reading the tour, or simply
          reading faster than it types, must be able to take the whole thing at
          once. Deliberately unlabelled: an instruction for it would be a third
          thing on screen competing with the sentence it is about. */}
      <div onClick={finish} style={{ position: 'absolute', inset: 0, cursor: 'default' }} />

      <button onClick={onSkip} className="mt-body-sm" style={{
        position: 'absolute', top: 18, right: 22,
        background: 'var(--t-ctrl)', border: '1px solid var(--t-ctrl-border)',
        borderRadius: 999, padding: '8px 16px', fontWeight: 700,
        color: 'var(--t-muted)', cursor: 'pointer',
      }}>{skipLabel}</button>

      <div style={{
        position: 'relative', height: '100%',
        display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center',
        padding: '80px 48px', textAlign: 'center', pointerEvents: 'none',
      }}>
        <div style={{ maxWidth: 1120 }}>
          {beat.mark && (
            <div style={{
              display: 'flex', flexDirection: 'column', alignItems: 'center', marginBottom: 26,
              animation: 'mer-take-rise .9s cubic-bezier(.2,.8,.25,1) both',
            }}>
              <Mark size={58} />
              <p style={{
                marginTop: 16, font: '750 30px var(--font-sans)',
                letterSpacing: '-.03em', color: 'var(--t-title)',
              }}>Meridian</p>
            </div>
          )}

          <div className="mt-label" style={{ color: 'var(--t-faint)', letterSpacing: '.18em' }}>
            {beat.kicker}
          </div>

          {/* The hero. `clamp` so it stays a hero on a 1280 tray window and does
              not become a wall on a 27in display. Weight 700 rather than the
              800 the rest of the app titles at: at this size 800 stops reading
              as emphasis and starts reading as a heavy face. */}
          <div style={{
            marginTop: 24,
            font: '700 clamp(38px,5vw,76px)/1.06 var(--font-sans)',
            letterSpacing: '-.025em',
            color: 'var(--t-title)',
            textWrap: 'pretty',
          }}>
            {shown}
            {!done && (
              <span aria-hidden style={{
                display: 'inline-block', width: 5, height: '.78em', marginLeft: 8,
                verticalAlign: '-.02em', background: 'var(--t-accent)',
                animation: 'mer-take-blink 1s step-end infinite',
              }} />
            )}
          </div>

          {/* Sub and CTA arrive together, once the hero has landed. Holding them
              back is what keeps the beat one thought at a time: a button visible
              under a half-typed sentence invites a press before the sentence has
              said anything. */}
          {done && (
            <div style={{ animation: `mer-take-fade ${REVEAL_MS}ms ease .1s both` }}>
              {beat.celebrate && <div style={{ marginTop: 26 }}><Confetti /></div>}
              {beat.sub && (
                <div className="mt-body" style={{
                  margin: '26px auto 0', maxWidth: 720,
                  fontSize: 19, lineHeight: 1.5, color: 'var(--t-muted)', textWrap: 'pretty',
                }}>{beat.sub}</div>
              )}
              {beat.cta && (
                <button onClick={onNext} className="mt-cta" style={{
                  pointerEvents: 'auto', marginTop: 40,
                  border: 0, borderRadius: 14, padding: '16px 32px',
                  fontSize: 15, fontWeight: 800, cursor: 'pointer',
                }}>{beat.cta}</button>
              )}
            </div>
          )}

          {/* Where we are, without saying how far. See `CHAPTERS`. */}
          <div style={{ marginTop: 36, display: 'flex', justifyContent: 'center', gap: 7 }}>
            {CHAPTERS.map((c, i) => (
              <span key={c.id} aria-hidden style={{
                width: 7, height: 7, borderRadius: 999,
                background: i <= at ? 'var(--t-accent)' : 'var(--t-hair)',
                opacity: i === at ? 1 : i < at ? 0.45 : 1,
                transition: 'background .3s ease, opacity .3s ease',
              }} />
            ))}
          </div>
        </div>
      </div>

      <style>{`
        @keyframes mer-take-rise { from { opacity:0; transform:translateY(12px) } to { opacity:1; transform:none } }
        @keyframes mer-take-blink { 0%,49% { opacity:1 } 50%,100% { opacity:0 } }
        @keyframes mer-take-fade { from { opacity:0; transform:translateY(8px) } to { opacity:1; transform:none } }
      `}</style>
    </div>
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
