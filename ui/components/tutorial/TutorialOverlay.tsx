//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The walkthrough's top layer: narration, animated cursor, spotlight ring, and
// the skip control. Sits above everything, including modals, so a beat can keep
// narrating while the user works inside the real planner or Settings.
//
// Takes no pointer events except on its own controls. The bottom-bubble
// narration must never swallow a click meant for the surface underneath — the
// spotlight works by RAISING its target, not by dimming everything else, and a
// scrim would fight that. The CENTRED variant is the one exception: it is a
// deliberate stop-and-read moment with nothing to click behind it.
//
// # Who calls this
// [`useTutorial`] renders it; `MeridianTimelineShell` mounts that.
//
// # Related
// - `./engine.ts` — the primitives whose state this draws

import { useEffect, useRef, useState } from 'react'
import { ProviderIcon } from '@/components/ProviderIcon'
import { availableTrackers } from '@/lib/integrations'
import { centreOf, tourTarget, type GhostCard, type StageChoice } from './engine'

/** Where the un-anchored narration bar sits, measured from the top of the viewport.
 *
 *  Sized to clear a modal's own title row rather than cover it: `ModalShell` insets the
 *  panel by `p-10` (40px) and its header is `py-5` around a single line, so the header
 *  ends near 104px. Landing on top of that would hide the modal's title and its × - the
 *  two things a user orients by, and the × is a control later beats actually wait on.
 *  With no modal open there is nothing at this height either, so one constant serves
 *  both cases. */
const HEADER_CLEAR = 116

/** Gap between the anchored bubble and its target, and a generous guess at the
 *  bubble's own height (three lines plus a row of choices). Shared by
 *  `AnchoredCaption` and the fits-beside test that decides whether to use it. */
const CAPTION_GAP = 16
const CAPTION_H = 130

/** Spotlight geometry — the ring and the fence hole it encloses, kept in one
 *  place because they are two drawings of the same shape and drifted apart
 *  once already, with nothing failing.
 *
 *  `RING_RADIUS` is deliberately rounder than the target's own corners. The
 *  ring sits `RING_INSET` outside its target, so matching the target's radius
 *  leaves the corners visibly tighter than the thing they enclose — which reads
 *  as a box drawn around the card rather than as the card being lit up.
 *
 *  `FENCE_PAD` is `Cutout`'s hole. It is the LARGER offset of the two, which is
 *  what made the corners a bug rather than a footnote: the hole was never under
 *  the ring, it stuck out past it, so its square corners showed as sharp
 *  un-dimmed nubs around a rounded card. The fix is `HOLE_RADIUS` — the dim now
 *  rounds itself instead of borrowing the ring's rounding. */
const RING_INSET = 6
const RING_RADIUS = 24
const FENCE_PAD = 8
/** Parallel to the ring rather than merely round: the hole sits
 *  `FENCE_PAD - RING_INSET` further out, so its radius grows by the same amount
 *  to keep the two curves concentric. Guarded by
 *  `__tests__/tutorial-spotlight-corners.test.ts`. */
const HOLE_RADIUS = RING_RADIUS + (FENCE_PAD - RING_INSET)

/** The tour's speaking voice, wherever it speaks.
 *
 *  `mt-body`'s spec, written out rather than worn as a class. It was briefly set
 *  larger and heavier on the reasoning that narration is the thing being read,
 *  and that was wrong on screen: at 15px/600 the bubble stops reading as the
 *  tour speaking beside the product and starts competing with the headings it
 *  is pointing at. The app's body size is the right register for a voice that
 *  has to sit next to real content without shouting over it.
 *
 *  What actually fixed the legibility complaint was the SURFACE, not the size -
 *  see `CAPTION_SURFACE`.
 *
 *  Held in one constant because the two variants (anchored beside a target, or
 *  parked at the top) are the same voice and drifted apart once already - one
 *  inverted, one not, at different sizes. */
// Bigger, heavier, and centred than it was (500 13px, left-aligned). At body
// weight and body size the tour's narration read as a caption on the UI rather
// than as someone talking the user through it, and it was competing for
// attention with the real interface text around it - which is denser, and
// which the user is also trying to read for the first time. The tour has to
// win that contest to be worth showing at all.
//
// Centred within the bubble, NOT moved to the centre of the screen. The bubble
// stays parked on its target on purpose (see the anchored variant below): an
// instruction read in one place and acted on in another is the failure this
// layout already exists to avoid.
const CAPTION_FONT: React.CSSProperties = {
  font: '650 16px/1.45 var(--font-sans)',
  letterSpacing: '-.011em',
  textAlign: 'center',
  color: 'var(--t-card)',
}

/** The surface the tour speaks from, shared by both bubbles.
 *
 *  INVERTED, so the narration reads as the GUIDE talking rather than as one more
 *  panel of the product. A light card here is a white box on a white page held
 *  apart by a hairline, which is what made the bar hard to pick out at all - and
 *  inside a modal, which is `--t-card` itself, it disappears outright.
 *
 *  It is not a light surface with a border instead. That was tried, on the
 *  reasoning that light type on a dark fill blooms under macOS font smoothing;
 *  the bloom is real but the cost was worse, because the bubble stopped looking
 *  like the tour and started looking like content. Kept at body weight (500)
 *  rather than something heavier, which is what actually makes inverted type
 *  soften. */
const CAPTION_SURFACE: React.CSSProperties = {
  background: 'var(--t-title)',
  border: '0.5px solid var(--t-title)',
  boxShadow: '0 10px 30px rgba(0,0,0,.28)',
}

/** Follows `selector` across scroll/resize so the ring and cursor stay put when
 *  the page moves under them. `null` parks both offscreen without unmounting,
 *  so the cursor keeps its transition and glides to its next target rather than
 *  teleporting. */
function useAnchor(selector: string | null) {
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null)
  useEffect(() => {
    if (!selector) { setPos(null); return }
    const measure = () => setPos(centreOf(selector))
    measure()
    // rAF loop rather than events: the targets move for reasons no event
    // reports (a modal's open transition, a card animating in), and the cost
    // is one getBoundingClientRect per frame while a walkthrough is running.
    let raf = 0
    const tick = () => { measure(); raf = requestAnimationFrame(tick) }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [selector])
  return pos
}

/** Box of `selector` in viewport coords, tracked the same way as `useAnchor`. */
/** Track a tour target's box, and whether it is CURRENTLY the thing being rung.
 *
 *  Two values, not one, and that split is what makes the ring glide instead of
 *  teleport. The script clears the spotlight between almost every pair of beats
 *  (`spotlight(x)` … `spotlight(null)` … `spotlight(y)`), and this used to answer
 *  `null` the instant that happened - which unmounted the ring, so the next beat
 *  mounted a brand-new element at the new coordinates. A freshly mounted element
 *  has nothing to transition FROM, so its `transition` was dead on arrival and
 *  every move was a hard cut, however long the duration said.
 *
 *  So the last known box SURVIVES the clear; only `live` goes false. The ring
 *  stays mounted at its old geometry, fades out, and when the next target
 *  arrives it animates across to it. `live` is what the caption logic reads,
 *  because "is something rung right now" is a different question from "where was
 *  the last thing rung". */
function useRect(selector: string | null): { rect: DOMRect | null; live: boolean } {
  const [state, setState] = useState<{ rect: DOMRect | null; live: boolean }>({ rect: null, live: false })
  useEffect(() => {
    if (!selector) { setState((s) => (s.live ? { rect: s.rect, live: false } : s)); return }
    let raf = 0
    const tick = () => {
      // Surface-scoped: the real dashboard is still mounted under the replica
      // and carries the same hooks, so a bare query rings the hidden original.
      const el = tourTarget(selector)
      const r = el ? el.getBoundingClientRect() : null
      setState((s) => (r ? { rect: r, live: true } : s.live ? { rect: s.rect, live: false } : s))
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [selector])
  return state
}

export function TutorialOverlay({ caption, centered, big, celebrate, cursorAt, cursorPoint, ghost, clicking, spotlight, spotlightDim, awaiting, choices, onChoose, onSkip, onSkipToDay = null }: {
  caption: string
  /** Render the top narration bar at headline size. Set for the whole of the day
   *  replay, which is the one stretch of the tour the user WATCHES rather than
   *  reads-then-acts - the ordinary bar is sized to sit quietly beside a control
   *  and disappears next to a nine-hour animation. */
  big: boolean
  /** Put confetti above a centred beat. Exactly one beat uses it, and that is the
   *  point: a tour that celebrates every step celebrates nothing. It is the LAST
   *  one - the only moment in a first run where "you are done" is literally true.
   *  See the closing beat of `scriptDay.ts`. */
  celebrate: boolean
  /** Render the narration as a CENTRED card rather than the bottom bubble.
   *  Used for the opening and closing beats, which are addressed to the user
   *  rather than to something on screen — a small bubble tucked at the bottom
   *  edge reads as a tooltip about the thing above it, which is exactly wrong
   *  for "welcome, here is what we are about to do". */
  centered: boolean
  cursorAt: string | null
  /** An explicit viewport point for the cursor, overriding `cursorAt`. Set only
   *  by `Stage.demoDrag`, which has to steer it to a spot inside a drop zone
   *  rather than to the centre of some element. */
  cursorPoint: { x: number; y: number } | null
  /** A clone of a real card, mid-flight across the screen. See `GhostCard`. */
  ghost: GhostCard | null
  clicking: boolean
  spotlight: string | null
  /** Blur and darken everything except the spotlit element, and swallow clicks
   *  outside it. See `Stage.spotlight` for when this is warranted. */
  spotlightDim: boolean
  /** True while a beat has handed control over — drives the "your turn" cue on
   *  the spotlight so a waiting user knows the app expects something of them. */
  awaiting: boolean
  /** Answers to the question in `caption`, when a beat is asking one. Always
   *  rendered with the caption, never as a detached dialog — the question and its
   *  answers are one thing, and separating them puts two focal points on screen.
   *  Which surface they land on is `centered`'s call: a real question centres and
   *  takes the screen, a one-button "carry on" stays inline. */
  choices: StageChoice[] | null
  onChoose: (value: string) => void
  /** `null` on the FIRST-RUN walkthrough, where there is deliberately no way to
   *  skip - see the call site in useTutorial. Non-null only for a replay the
   *  user asked for. */
  onSkip: (() => void) | null
  /** DEV ONLY: jump to part two. `null` in a packaged build, where the pill is
   *  not rendered - see the button below. */
  onSkipToDay?: (() => void) | null
}) {
  // A centred beat owns the whole screen: the scrim below hides the page, so a
  // ring drawn around something nobody can see and a cursor gliding toward it
  // are pure noise on top of a question. Suppressed at the source rather than
  // per-element so the anchored caption cannot resurface either.
  const anchored = useAnchor(centered || cursorPoint ? null : cursorAt)
  const cursor = centered ? null : (cursorPoint ?? anchored)
  // `ringBox` is where the ring IS (kept across a clear, so it can glide to the
  // next target); `ring` is what is being rung right NOW, which is the question
  // every caption decision below is asking.
  const { rect: ringBox, live: ringLive } = useRect(centered ? null : spotlight)
  const ring = ringLive ? ringBox : null
  // Can the bubble actually park beside this target? For an ordinary control
  // yes, and the anchored variant is always right. For a target that fills the
  // window - the beat that rings the WHOLE day view - there is no room above and
  // "below" is off the bottom of the screen, so the anchored caption renders
  // where nobody can read it and the beat looks like it stopped narrating.
  // Those fall back to the top bar, which is the variant for beats with nothing
  // small to point at, and is exactly what a whole-screen ring is.
  const ringFits = ring !== null && (ring.top - CAPTION_GAP - CAPTION_H > 0
    || ring.bottom + CAPTION_GAP + CAPTION_H < window.innerHeight)
  const unanchored = !!caption && !centered && !(ring && ringFits)

  // TELL THE PAGE WHERE THIS BAR ENDS, so a modal under it can move its own
  // content out of the way (`ModalShell`'s TOUR_BODY_PAD).
  //
  // The bar floats over the page at a fixed height picked to clear a modal's
  // title row - which it does, and then lands on the first line of the body 28px
  // below. Nothing here can fix that alone: the bar and the panel are both
  // centred with overlapping max-widths, and the two modals this happens on are
  // `scrollInside` ones that can fill the window, so there is no free space to
  // move the bar into. Publishing the measurement lets the party that CAN make
  // room do it.
  //
  // Measured, not derived: the bar is one to three lines depending on the beat,
  // and `big` changes its font and padding, so a constant would be wrong for
  // most captions.
  //
  // OBSERVED rather than re-measured on a dependency list, because the caption
  // is not the only thing that changes this height and the other two are the
  // easy ones to forget. The bar is capped at `maxWidth: big ? 760 : 540`, so a
  // window narrower than that re-wraps the text on a resize with no prop
  // changing at all; and `choices` render INSIDE this bar, so a beat that adds
  // buttons under an unchanged caption grows it too. Either way the published
  // value would stay stale until the next `say()` - and a stale value is worse
  // than none, because `ModalShell` pads to it and would hold a gap that no
  // longer matches anything on screen. A `ResizeObserver` covers all three
  // sources and the first measurement, so nothing has to be enumerated.
  const sayRef = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    const root = document.documentElement
    const clear = () => root.style.removeProperty('--mer-tour-say-bottom')
    const el = sayRef.current
    if (!unanchored || !el) {
      clear()
      return
    }
    // `bottom`, not `height`: the property names where the bar ENDS in the
    // viewport, which is what a modal has to clear. The bar's top is fixed by
    // CSS, so a size change is the only thing that can move it.
    const publish = () => {
      const box = el.getBoundingClientRect()
      root.style.setProperty('--mer-tour-say-bottom', `${Math.round(box.bottom)}px`)
    }
    const ro = new ResizeObserver(publish)
    ro.observe(el) // fires once immediately, so this is also the initial read
    return () => {
      ro.disconnect()
      // The tour is the only writer of this property, so clearing on the way
      // out is what stops a finished walkthrough leaving every modal padded.
      clear()
    }
  }, [unanchored])

  return (
    <div className="fixed inset-0" style={{ zIndex: 9000, pointerEvents: 'none' }}>
      {/* The card being dragged, under the cursor and over everything else.
          A CLONE of the real row's markup rather than a drawn stand-in: the board
          holds the user's own tickets, and a demonstration that flies a made-up
          card across their screen is showing them someone else's data. Inert -
          `pointer-events: none` on the wrapper - so it cannot intercept the drop
          it is illustrating. */}
      {ghost && (
        <div style={{
          position: 'absolute', left: ghost.left, top: ghost.top,
          width: ghost.width, height: ghost.height,
          transform: `translate(${ghost.dx}px, ${ghost.dy}px) rotate(${ghost.dx || ghost.dy ? -1.6 : 0}deg) scale(1.02)`,
          // Matched to the cursor's transition exactly, so the card stays under
          // the pointer for the whole trip instead of arriving before or after it.
          transition: 'transform 1s cubic-bezier(.2,.8,.25,1)',
          boxShadow: '0 22px 44px -14px rgba(40,30,90,0.45)',
          borderRadius: 8, overflow: 'hidden',
          opacity: 0.97, pointerEvents: 'none',
        }}>
          {/* Our own markup, one frame old - not user input, and never rendered
              anywhere the user could have authored the HTML. */}
          <div dangerouslySetInnerHTML={{ __html: ghost.html }} />
        </div>
      )}

      {/* The dimmed variant: four blurred panels fenced around the target,
          leaving it sharp, lit and the only clickable thing on screen.
          Rendered BEFORE the ring and cursor so those stay crisp on top. */}
      {/* THE FENCE. While the tour is waiting on one specific click, everything
          outside the spotlight stops taking clicks.

          Without it the only beat that protected itself was the dimmed one,
          because blocking was a side effect of the blur rather than a thing
          anyone had asked for. Every other beat left the whole app live behind
          a ring - including the close X on whatever modal the tour had just
          opened. Clicking it does not fail: the modal closes, the tour keeps
          waiting for a click on a target that is no longer on screen, and the
          user is left reading an instruction about a window they cannot see.

          Gated on `awaiting` on purpose. A fence that stands during narration
          beats would make the app feel frozen for reasons the user cannot see;
          one that stands only while the tour is genuinely blocked on them is
          the difference between guiding and trapping.

          Rendered BEFORE the caption, the choice buttons and Skip, so those
          later siblings paint on top and keep their own `pointerEvents:'auto'`.
          The way out is always one click away. */}
      {ring && spotlightDim && <Cutout rect={ring} dim />}
      {ring && !spotlightDim && awaiting && <Cutout rect={ring} dim={false} />}

      {/* Spotlight — a ring around the real control. On its own (the default)
          it adds no scrim at all: the target stays fully interactive because
          this sits above it but takes no pointer events. */}
      {/* MOUNTED FOR THE WHOLE RUN once the first beat has rung anything, and
          hidden with opacity rather than removed. Unmounting was what made every
          move a hard cut: the ring the next beat drew was a different element,
          with no previous geometry to animate away from. Geometry moves slower
          than the fade so a beat-to-beat hop reads as one object travelling,
          while the fade keeps it from streaking across the screen when the next
          target is on the other side of the window. */}
      {ringBox && (
        <div className="absolute" style={{
          left: ringBox.left - RING_INSET, top: ringBox.top - RING_INSET,
          width: ringBox.width + RING_INSET * 2, height: ringBox.height + RING_INSET * 2,
          borderRadius: RING_RADIUS,
          border: `2px solid var(--t-accent)`,
          boxShadow: '0 0 0 4px color-mix(in srgb, var(--t-accent) 22%, transparent)',
          opacity: ringLive ? 1 : 0,
          transition: 'left .5s cubic-bezier(.22,1,.32,1), top .5s cubic-bezier(.22,1,.32,1),'
            + ' width .5s cubic-bezier(.22,1,.32,1), height .5s cubic-bezier(.22,1,.32,1),'
            + ' opacity .3s ease',
          animation: awaiting && ringLive ? 'mer-tour-pulse 1.6s ease-in-out infinite' : undefined,
        }} />
      )}

      {/* Animated cursor. Parked offscreen when idle rather than unmounted, so
          its CSS transition survives and it glides between targets. */}
      <div className="absolute" style={{
        transform: `translate(${cursor?.x ?? -200}px, ${cursor?.y ?? -200}px)`,
        transition: 'transform 1s cubic-bezier(.2,.8,.25,1)',
        opacity: cursor ? 1 : 0,
      }}>
        <div style={{
          width: 22, height: 22, marginLeft: -4, marginTop: -2,
          transform: clicking ? 'scale(.82)' : 'scale(1)',
          transition: 'transform .16s',
        }}>
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
            <path d="M5 2l14 11-6.5.6L15 20l-2.6 1.1L9 14.2 5 17.6z"
              fill="#fff" stroke="rgba(0,0,0,.55)" strokeWidth="1.2" strokeLinejoin="round" />
          </svg>
        </div>
        {clicking && (
          <span className="absolute" style={{
            left: -6, top: -6, width: 34, height: 34, borderRadius: 99,
            border: '2px solid var(--t-accent)',
            animation: 'mer-tour-ping .5s ease-out',
          }} />
        )}
      </div>

      {/* Narration, pinned bottom-centre so it never covers the timeline column
          or the right panel the beats point at. */}
      {caption && centered && (
        // A real scrim, not a tint. At 45%/2px the planner behind stayed legible
        // and the card read as one more panel floating over a busy screen - so a
        // question meant to stop the user competed with the board they were
        // already reading. Opaque enough that there is nothing else to look at,
        // with the blur doing the rest: the page is still THERE (this is a pause,
        // not a navigation) but it has nothing left to say.
        <div className="absolute inset-0 flex items-center justify-center p-10"
          style={{
            // A LITERAL rgba, not a token. `--win-bg` is a linear-gradient, so
            // `color-mix(in srgb, var(--win-bg) …)` is invalid, the declaration is
            // dropped whole, and the scrim silently disappears - leaving only the
            // blur, which washes the page without hiding it. Same base colour as
            // `ModalShell`'s 0.5 backdrop, taken far darker because this one has to
            // cover an open modal rather than the page behind it.
            background: 'rgba(20,16,40,0.88)',
            backdropFilter: 'blur(14px)', WebkitBackdropFilter: 'blur(14px)',
            pointerEvents: 'auto',
            animation: 'mer-tour-dim .28s ease both',
          }}>
          <div className="mer-pop text-center" style={{
            // Wider when the answers carry artwork - two illustrated cards side by
            // side need the room, and at 520 they wrapped to a column, which read
            // as a list of options rather than as a fork.
            maxWidth: choices?.some(c => c.art) ? 640 : 520,
            padding: '30px 34px 26px', borderRadius: 20,
            background: 'var(--t-card)',
            border: '0.5px solid var(--t-card-border)',
            boxShadow: 'var(--mt-modal-shadow)',
          }}>
            {celebrate && <Confetti />}
            {/* No eyebrow label. The centred card is used for the closing beat,
                where a "Getting started" tag contradicts the sentence under it —
                and the opening is now the full-screen title sequence, which
                frames the tour far better than a two-word tag ever did. */}
            {/* Deliberately the SAME spec as `TutorialIntro`'s lead line, not a
                size picked for this card: the title sequence is the last type the
                user saw, and the tour's own voice should not change font halfway
                through. Off-scale sizes with no tracking were what made this read
                as a different product from the page under it. */}
            <p style={{
              font: '600 19px/1.4 var(--font-sans)', letterSpacing: '-.01em',
              color: 'var(--t-title)', textWrap: 'pretty',
            }}>
              {caption}
            </p>
            {choices && choices.length > 0 && (
              <div className="flex gap-3 mt-6 justify-center flex-wrap">
                {/* Illustrated answers are NEVER primary-filled. `primary` styles
                    the first option forward, which is fine for a "carry on"
                    button and wrong for a fork: a violet card beside a plain one
                    makes the plain one read as the wrong answer, and the artwork
                    would be sitting on a solid accent besides. Two equal cards is
                    also the honest rendering - the tour has no view on which the
                    user is. */}
                {choices.map((c, i) => (
                  <ChoiceButton key={c.value} choice={c} primary={i === 0 && !c.art} onChoose={onChoose} />
                ))}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Anchored variant — used for EVERY beat with a spotlight, not just the
          dimmed one. A bottom-edge bubble makes the user read an instruction in
          one corner and act in another, and inside a modal it lands outside the
          surface it is talking about entirely. Parked directly above the target,
          the sentence and the thing it names are one glance. The bottom bubble
          survives only for beats with nothing to point at. */}
      {caption && !centered && ring && ringFits && (
        <AnchoredCaption rect={ring}>
          {/* Keyed on the text so a new sentence re-runs the fade. Without it the
              words swapped in place between beats - the bubble stayed put and its
              contents simply became different, which is the change the eye is
              least likely to notice and most likely to find abrupt. */}
          <p key={caption} style={{
            ...CAPTION_FONT, textWrap: 'pretty',
            animation: 'mer-tour-say .3s cubic-bezier(.2,.8,.25,1) both',
          }}>
            {caption}
          </p>
          {choices && choices.length > 0 && (
            // AT THE END OF THE READING PATH, not under the first word. The
            // button that advances is what you reach for AFTER the sentence, so
            // it belongs where the sentence finishes - the same place every
            // dialog in the app puts its forward action. Left-aligned it sat
            // back at the start, so the eye crossed the bubble twice.
            <div className="flex justify-end gap-2 mt-3 flex-wrap">
              {choices.map((c, i) => <ChoiceButton key={c.value} choice={c} primary={i === 0} onChoose={onChoose} />)}
            </div>
          )}
        </AnchoredCaption>
      )}

      {/* The un-anchored bubble — beats with nothing to point at.
          AT THE TOP, AND DELIBERATELY NOT A CARD. Two separate problems, one
          box. It used to sit at `bottom: 26`, which on a tall modal put the
          instruction below everything it was talking about: the user read the
          last thing on screen first, and on the plan modal it landed under the
          board, well outside the reading path. And it was `--t-card` on a
          `--t-card` modal with a hairline border - a white box on white, which
          is what makes it hard to read rather than merely low.
          So it moves to the top of the reading order, and it stops pretending
          to be part of the page: an opaque, inverted surface that reads as the
          GUIDE talking, distinct from every real card under it. `HEADER_CLEAR`
          keeps it off the modal's own title row and × instead of covering the
          controls the next beat may need.
          The ANCHORED variant above is untouched: it parks beside its target on
          purpose, and moving it up here would put the sentence and the thing it
          names at opposite ends of the screen. */}
      {unanchored && (
        // TWO divs, and they cannot be collapsed into one. `.mer-pop` animates
        // `transform` with fill `both`, so it OVERWRITES an inline
        // `translateX(-50%)` for the element's whole life - the bar rendered half
        // its own width to the right of centre, overlapping the modal beside it.
        // Centring lives on the outer div, the animation on the inner, so the two
        // transforms stop fighting over one property.
        <div ref={sayRef} className="absolute" style={{
          // NARROWER THAN IT WAS (620). At body size a full-width line ran to
          // ~95 characters, well past the ~65 the eye tracks without losing its
          // place - so a two-sentence beat read as a paragraph even after the
          // sentences themselves were cut back.
          top: HEADER_CLEAR, left: '50%', transform: 'translateX(-50%)',
          pointerEvents: 'auto', maxWidth: big ? 760 : 540,
        }}>
        <div className="mer-pop" style={{
          padding: big ? '18px 28px' : '13px 18px', borderRadius: big ? 19 : 15,
          ...CAPTION_SURFACE,
        }}>
          <p key={caption} style={{
            ...CAPTION_FONT, textWrap: 'pretty',
            animation: 'mer-tour-say .3s cubic-bezier(.2,.8,.25,1) both',
            ...(big ? { font: '700 21px/1.35 var(--font-sans)', letterSpacing: '-.015em', textAlign: 'center' } : null),
          }}>
            {caption}
          </p>
          {choices && choices.length > 0 && (
            // AT THE END OF THE READING PATH, not under the first word. The
            // button that advances is what you reach for AFTER the sentence, so
            // it belongs where the sentence finishes - the same place every
            // dialog in the app puts its forward action. Left-aligned it sat
            // back at the start, so the eye crossed the bubble twice.
            <div className="flex justify-end gap-2 mt-3 flex-wrap">
              {choices.map((c, i) => <ChoiceButton key={c.value} choice={c} primary={i === 0} onChoose={onChoose} />)}
            </div>
          )}
        </div>
        </div>
      )}

      {/* Skip — on EVERY run, first run included. This used to be replays only,
          on the argument that an escape hatch on the first screen a new user
          ever sees reads as the recommended action to anyone unsure. That was
          reversed once a tour proved able to arrive unrequested, on a mount days
          after it was armed, with closing the window as the only way out. The
          full reasoning for the reversal is at the call site in useTutorial.

          Still conditional rather than unconditional: the prop is the seam, and
          a caller that has a reason to withhold it should be able to.

          When it IS shown it is its own control, NOT inside the caption bubble.
          It used to live in there, which meant it vanished on any beat with an
          empty caption and moved horizontally as the caption's width changed.
          An escape hatch that relocates, or disappears exactly when a confused
          user reaches for it, is not an escape hatch. Fixed top-right, present
          for the entire run. */}
      {onSkip && (
        <button onClick={onSkip} className="absolute mt-body-sm" style={{
          top: 14, right: 16, pointerEvents: 'auto',
          color: 'var(--t-muted)', cursor: 'pointer',
          padding: '6px 13px', borderRadius: 99,
          background: 'var(--t-card)',
          border: '0.5px solid var(--t-card-border)',
          boxShadow: 'var(--pop-shadow)',
        }}>Skip tour</button>
      )}

      {/* DEV ONLY - `null` in a packaged build, so this is not rendered at all.
          Jumps straight to part two (the timeline rebuilding itself), because
          reaching that half honestly costs a daily plan, an OAuth round-trip and
          a provider install, which is how a walkthrough stops getting edited.
          It is NOT shipped: it skips the compulsory AI-connect step the rest of
          the product depends on.
          Parked to the LEFT of Skip rather than beside it in a group: Skip is a
          fixed landmark for the whole run (see above), and a dev control that
          shifts it would move the real escape hatch every time the build changes. */}
      {onSkipToDay && (
        <button onClick={onSkipToDay} className="absolute mt-body-sm" style={{
          top: 14, right: 116, pointerEvents: 'auto',
          color: 'var(--t-accent)', cursor: 'pointer',
          padding: '6px 13px', borderRadius: 99,
          background: 'var(--t-card)',
          border: '0.5px dashed color-mix(in srgb, var(--t-accent) 55%, transparent)',
          boxShadow: 'var(--pop-shadow)',
        }}>Skip to the day</button>
      )}

      <style>{`
        @keyframes mer-tour-pulse {
          0%,100% { box-shadow: 0 0 0 4px color-mix(in srgb, var(--t-accent) 22%, transparent); }
          50%     { box-shadow: 0 0 0 9px color-mix(in srgb, var(--t-accent) 8%, transparent); }
        }
        @keyframes mer-tour-dim { from { opacity: 0 } to { opacity: 1 } }
        @keyframes mer-tour-rise {
          from { opacity: 0; transform: translateY(9px) scale(.94) }
          to   { opacity: 1 }
        }
        @keyframes mer-tour-party {
          0%   { opacity: 0; transform: scale(.4) rotate(-18deg) }
          55%  { opacity: 1; transform: scale(1.16) rotate(6deg) }
          100% { opacity: 1; transform: scale(1) rotate(0) }
        }
        @keyframes mer-tour-fleck {
          0%   { opacity: 0; transform: translate(0,0) rotate(0) }
          22%  { opacity: 1 }
          100% { opacity: 0; transform: translate(var(--dx), var(--dy)) rotate(var(--rot)) }
        }
        @keyframes mer-tour-ping {
          from { transform: scale(.6); opacity: .9 }
          to   { transform: scale(1.5); opacity: 0 }
        }
        /* One sentence giving way to the next. Small on purpose: the bubble does
           not move, so anything larger reads as the bubble itself jumping. */
        @keyframes mer-tour-say {
          from { opacity: 0; transform: translateY(4px) }
          to   { opacity: 1; transform: none }
        }
        @media (prefers-reduced-motion: reduce) {
          [style*="mer-tour-say"] { animation: none !important }
        }
      `}</style>
    </div>
  )
}

/** A caption bubble parked against `rect` — ABOVE it by preference, below when
 *  there is not enough room above, and clamped so it never runs off either edge.
 *
 *  BELOW whenever below fits; above only when it does not.
 *
 *  This used to prefer above, on the reasoning that a caption is an instruction
 *  for the thing underneath it - read the line, then act on what it sits on.
 *  That reads well for a lone button, and badly for anything with a heading,
 *  which is most of the product: the space immediately above a labelled block is
 *  occupied by the label, so "above" reliably covers the one word naming what the
 *  caption is talking about. Both detail-panel beats hit this - the bubble
 *  explaining WHEN sat on top of the words "When you did it".
 *
 *  Below is empty far more often, and where it is not, what it covers is content
 *  the beat has not got to yet rather than the title of what it is discussing.
 *  Controls near the bottom of the window still flip up, since below genuinely
 *  does not fit there. */
function AnchoredCaption({ rect, children }: { rect: DOMRect; children: React.ReactNode }) {
  const W = 330
  const GAP = CAPTION_GAP
  const vw = typeof window === 'undefined' ? 1200 : window.innerWidth
  const vh = typeof window === 'undefined' ? 800 : window.innerHeight
  const fitsAbove = rect.top - GAP - CAPTION_H > 0
  const fitsBelow = rect.bottom + GAP + CAPTION_H < vh
  const above = fitsAbove && !fitsBelow
  const left = Math.min(Math.max(12, rect.left + rect.width / 2 - W / 2), vw - W - 12)
  return (
    <div className="absolute mer-pop" style={{
      left, width: W,
      ...(above ? { bottom: vh - rect.top + GAP } : { top: rect.bottom + GAP }),
      pointerEvents: 'auto',
      padding: '13px 16px', borderRadius: 15,
      // One surface for both bubbles - see CAPTION_SURFACE for why it is a light
      // card rather than the inverted slab this used to be.
      ...CAPTION_SURFACE,
      // Matched to the ring's own travel, so the bubble and the thing it is
      // parked against arrive together rather than one chasing the other.
      transition: 'top .5s cubic-bezier(.22,1,.32,1), bottom .5s cubic-bezier(.22,1,.32,1),'
        + ' left .5s cubic-bezier(.22,1,.32,1)',
    }}>{children}</div>
  )
}

/** The blur-everything-else layer, as four panels fenced around `rect` rather
 *  than one masked full-screen pane.
 *
 *  A single pane with an SVG-masked hole is the tidier construction, but
 *  `backdrop-filter` under a mask is exactly the combination WebKit has been
 *  unreliable about, and this ships inside a WKWebView on macOS. Four plain
 *  rectangles are boring and render identically everywhere.
 *
 *  The cost is that the hole comes out square. This file used to claim that did
 *  not matter, because "the spotlight ring drawn on top is rounded and reads as
 *  the frame" — but the ring is drawn at `RING_INSET` and the hole at the LARGER
 *  `FENCE_PAD`, so the hole was never under the ring to begin with. Each corner
 *  sat ~13px outside the ring's arc and showed as a sharp un-dimmed nub around a
 *  rounded card.
 *
 *  So the dim no longer rides on the panes. It is a separate scrim laid over the
 *  hole with a `box-shadow` spread big enough to reach past any viewport, which
 *  is the one construction that gives a rounded hole without a mask:
 *  `box-shadow` honours `border-radius`, and its overflow is ink rather than
 *  layout, so a spread far outside the parent costs nothing and scrolls nothing.
 *
 *  The BLUR boundary stays square — `backdrop-filter` is clipped to the pane's
 *  own box. That is deliberate and not a residual bug: blur is only visible
 *  where there is detail to smear, and the corner nubs are a few pixels of the
 *  window's background gradient. One beat in the whole script dims at all
 *  (`script.ts`'s `plan-open`), so this was checked rather than assumed.
 *
 *  These DO take pointer events, which is half the point — while the tour is
 *  waiting on one specific click, a stray click into the blurred area should
 *  land on nothing rather than on some control the user cannot see clearly.
 *
 *  `dim: false` renders the same four panes completely invisible — see
 *  [`Cutout`]'s note on why the fence and the blur are one component. */
function Cutout({ rect, dim }: { rect: DOMRect; dim: boolean }) {
  const l = Math.max(0, rect.left - FENCE_PAD)
  const t = Math.max(0, rect.top - FENCE_PAD)
  const r = rect.right + FENCE_PAD
  const b = rect.bottom + FENCE_PAD
  // The panes travel with the hole rather than being re-laid instantly, so a
  // dimmed beat handing over to the next one reads as the light moving. The
  // scrim below has to carry the SAME easing and duration, or the dim snaps
  // while the blur slides after it.
  const glide = 'left .5s cubic-bezier(.22,1,.32,1), top .5s cubic-bezier(.22,1,.32,1),'
    + ' width .5s cubic-bezier(.22,1,.32,1), height .5s cubic-bezier(.22,1,.32,1)'
  const pane: React.CSSProperties = {
    position: 'absolute',
    pointerEvents: 'auto',
    ...(dim ? {
      backdropFilter: 'blur(5px)',
      WebkitBackdropFilter: 'blur(5px)',
      // The same fade the scrim uses. The blur and the dim are one effect to
      // the eye, so they have to arrive together - splitting the animation off
      // with the colour would snap the blur in and then darken it.
      animation: 'mer-tour-dim .45s ease both',
    } : null),
    transition: glide,
  }
  return (
    <>
      <div style={{ ...pane, left: 0, top: 0, right: 0, height: t }} />
      <div style={{ ...pane, left: 0, top: b, right: 0, bottom: 0 }} />
      <div style={{ ...pane, left: 0, top: t, width: l, height: b - t }} />
      <div style={{ ...pane, left: r, top: t, right: 0, height: b - t }} />
      {/* The dim, as a rounded hole. Inert - the four panes above are what
          catch stray clicks, and this must not take the click the beat is
          waiting for.

          Literal rgba for the same reason as the centred scrim: `--win-bg` is a
          gradient, so a `color-mix` against it is invalid and drops the
          declaration outright. This read as "blur only, no dimming" for as long
          as it lived on the panes - they were doing half their job silently. */}
      {/* Deliberately NOT the clamped `l`/`t` the panes use. The clamp exists
          because a pane sized `height: t` cannot take a negative number; the
          scrim has no such problem, and applying the clamp to it would matter
          in a way it never did to the panes. A target within `FENCE_PAD` of
          the top or left edge would pull one side of the scrim inside the ring
          while it kept its rounding - a dimmed notch cutting into the lit area,
          which is the same class of artifact this whole change removes. Letting
          it run off-screen instead keeps the curve concentric everywhere; the
          part outside the viewport simply is not painted. */}
      {dim && (
        <div style={{
          position: 'absolute', pointerEvents: 'none',
          left: rect.left - FENCE_PAD, top: rect.top - FENCE_PAD,
          width: rect.width + FENCE_PAD * 2, height: rect.height + FENCE_PAD * 2,
          borderRadius: HOLE_RADIUS,
          boxShadow: '0 0 0 9999px rgba(20,16,40,0.42)',
          animation: 'mer-tour-dim .45s ease both',
          transition: glide,
        }} />
      )}
    </>
  )
}

/** One answer button. `primary` is the styled-forward one — not a
 *  recommendation, but the beats that offer a choice put the branch with more to
 *  configure first, so the heavier path reads as available rather than buried. */
function ChoiceButton({ choice, primary, onChoose }: {
  choice: StageChoice
  primary: boolean
  onChoose: (value: string) => void
}) {
  const art = choice.art
  return (
    <button onClick={() => onChoose(choice.value)}
      className={`mt-card-hover ${art ? 'text-center' : 'text-left'}`}
      style={{
        padding: art ? '20px 18px 16px' : '9px 16px',
        borderRadius: art ? 16 : 11, cursor: 'pointer',
        ...(art ? { width: 254, maxWidth: '100%' } : null),
        background: primary ? 'var(--btn-primary-bg)' : 'var(--t-box)',
        color: primary ? '#fff' : 'var(--t-title)',
        border: primary ? 'none' : '1px solid var(--t-card-border)',
      }}>
      {art && <ChoiceArt kind={art} />}
      {/* Card-title + body-sm, the same pair every real card in the app uses for
          a heading over its supporting line — so an answer here looks like
          something the product offers, not like tour chrome. */}
      <span className="mt-card-title" style={{ display: 'block' }}>{choice.label}</span>
      {choice.hint && (
        <span className="mt-body-sm" style={{
          display: 'block', marginTop: art ? 4 : 2,
          color: primary ? 'rgba(255,255,255,.82)' : 'var(--t-muted)',
        }}>{choice.hint}</span>
      )}
    </button>
  )
}

/** The one celebration in the tour: a 🎉 that pops in, with a scatter of flecks
 *  thrown off it.
 *
 *  Flecks are plain coloured rectangles rather than more emoji - a burst of
 *  party-poppers reads as a template, and the coloured paper reads as the app's
 *  own. Each one's throw is derived from its index, not randomised, so the burst
 *  is identical on every run and on a replay: a first-run screen that looks
 *  different each time it is seen is a first-run screen that looks unfinished. */
function Confetti() {
  const HUES = ['var(--t-accent)', 'var(--color-state-approved)', 'var(--color-state-pending)']
  return (
    <span className="relative block mx-auto mb-4" style={{ width: 74, height: 62 }} aria-hidden>
      {Array.from({ length: 12 }, (_, i) => {
        const a = (i / 12) * Math.PI * 2
        return (
          <span key={i} className="absolute" style={{
            left: 35, top: 26, width: 6, height: 9, borderRadius: 1.5,
            background: HUES[i % HUES.length],
            // Custom properties feed the keyframes, so twelve flecks share one
            // animation rather than needing twelve of them.
            ['--dx' as string]: `${Math.cos(a) * 62}px`,
            ['--dy' as string]: `${Math.sin(a) * 62 - 18}px`,
            ['--rot' as string]: `${i * 63}deg`,
            animation: `mer-tour-fleck 1.15s cubic-bezier(.2,.7,.3,1) ${180 + (i % 4) * 60}ms both`,
          }} />
        )
      })}
      <span className="absolute" style={{
        left: 0, right: 0, top: 0, fontSize: 46, lineHeight: '58px',
        animation: 'mer-tour-party .62s cubic-bezier(.2,.9,.25,1) both',
      }}>🎉</span>
    </span>
  )
}

/** The illustration above an answer's label.
 *
 *  REAL BRAND MARKS, not a generic clipboard glyph. The whole point is that
 *  someone who lives in Jira recognises their own tool before they have read
 *  anything — a made-up "board" icon would be one more abstraction to decode, and
 *  we already ship the actual logos for the ticket lists (`ProviderIcon`).
 *
 *  Both variants are the same construction (a floating tile, or three of them)
 *  so the two answers read as a matched pair rather than as one illustrated
 *  option beside a decorated one. The stagger is what makes it read as a
 *  deliberate flourish rather than as three images loading. */
function ChoiceArt({ kind }: { kind: NonNullable<StageChoice['art']> }) {
  const tile: React.CSSProperties = {
    background: 'var(--t-card)',
    border: '1px solid var(--t-card-border)',
    boxShadow: '0 8px 18px -10px rgba(40,30,90,.5)',
  }
  // DERIVED, never a hardcoded triple. This is a first-run screen showing brand
  // marks as an offer, so a logo here for something you cannot actually connect
  // is the worst kind of wrong - the user picks this card BECAUSE they saw their
  // tool, then finds it greyed out as "Coming soon" two beats later. Linear is
  // flagged exactly that way today.
  const offered = availableTrackers()
  return (
    <span className="flex items-end justify-center gap-2 mb-3.5" style={{ height: 46 }}>
      {kind === 'trackers'
        ? offered.map((t, i) => (
          <span key={t.id} className="inline-flex items-center justify-center rounded-xl" style={{
            ...tile, width: 40, height: 40,
            // The middle one sits slightly proud, so an odd row reads as a group
            // rather than as a toolbar. An even row stays level - raising one of a
            // pair just looks like a mistake.
            transform: offered.length % 2 === 1 && i === (offered.length - 1) / 2 ? 'translateY(-5px)' : undefined,
            animation: `mer-tour-rise .5s cubic-bezier(.2,.8,.25,1) ${i * 90}ms both`,
          }}>
            <ProviderIcon provider={t.id} size={21} />
          </span>
        ))
        : (
          <span className="rounded-xl flex flex-col justify-center gap-[5px] px-3" style={{
            ...tile, width: 92, height: 45,
            animation: 'mer-tour-rise .5s cubic-bezier(.2,.8,.25,1) both',
          }}>
            {[0, 1, 2].map(i => (
              <span key={i} className="flex items-center gap-1.5" style={{
                animation: `mer-tour-rise .4s cubic-bezier(.2,.8,.25,1) ${140 + i * 110}ms both`,
              }}>
                <svg width="10" height="10" viewBox="0 0 12 12" fill="none" aria-hidden>
                  {/* The last row is unticked - a list you are still working
                      through, which is what a day's plan actually looks like. */}
                  <path d="M1.8 6.3l2.7 2.7L10.2 3.2" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"
                    stroke={i === 2 ? 'var(--t-hair)' : 'var(--color-state-approved)'} />
                </svg>
                <span style={{ height: 3.5, borderRadius: 9, flex: 1, background: 'var(--t-hair)' }} />
              </span>
            ))}
          </span>
        )}
    </span>
  )
}
