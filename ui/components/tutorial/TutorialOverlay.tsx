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

import { useEffect, useState } from 'react'
import { centreOf, type StageChoice } from './engine'

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
function useRect(selector: string | null) {
  const [rect, setRect] = useState<DOMRect | null>(null)
  useEffect(() => {
    if (!selector) { setRect(null); return }
    let raf = 0
    const tick = () => {
      const el = document.querySelector(selector)
      setRect(el ? el.getBoundingClientRect() : null)
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [selector])
  return rect
}

export function TutorialOverlay({ caption, centered, cursorAt, clicking, spotlight, spotlightDim, awaiting, choices, onChoose, onSkip }: {
  caption: string
  /** Render the narration as a CENTRED card rather than the bottom bubble.
   *  Used for the opening and closing beats, which are addressed to the user
   *  rather than to something on screen — a small bubble tucked at the bottom
   *  edge reads as a tooltip about the thing above it, which is exactly wrong
   *  for "welcome, here is what we are about to do". */
  centered: boolean
  cursorAt: string | null
  clicking: boolean
  spotlight: string | null
  /** Blur and darken everything except the spotlit element, and swallow clicks
   *  outside it. See `Stage.spotlight` for when this is warranted. */
  spotlightDim: boolean
  /** True while a beat has handed control over — drives the "your turn" cue on
   *  the spotlight so a waiting user knows the app expects something of them. */
  awaiting: boolean
  /** Answers to the question in `caption`, when a beat is asking one. Rendered
   *  inside the caption bubble rather than as a separate dialog: the question is
   *  the narration, and splitting them would put two competing focal points on
   *  screen at once. */
  choices: StageChoice[] | null
  onChoose: (value: string) => void
  onSkip: () => void
}) {
  const cursor = useAnchor(cursorAt)
  const ring = useRect(spotlight)

  return (
    <div className="fixed inset-0" style={{ zIndex: 9000, pointerEvents: 'none' }}>
      {/* The dimmed variant: four blurred panels fenced around the target,
          leaving it sharp, lit and the only clickable thing on screen.
          Rendered BEFORE the ring and cursor so those stay crisp on top. */}
      {ring && spotlightDim && <DimCutout rect={ring} />}

      {/* Spotlight — a ring around the real control. On its own (the default)
          it adds no scrim at all: the target stays fully interactive because
          this sits above it but takes no pointer events. */}
      {ring && (
        <div className="absolute" style={{
          left: ring.left - 6, top: ring.top - 6,
          width: ring.width + 12, height: ring.height + 12,
          borderRadius: 18,
          border: `2px solid var(--color-state-proposal)`,
          boxShadow: '0 0 0 4px color-mix(in srgb, var(--color-state-proposal) 22%, transparent)',
          transition: 'all .28s cubic-bezier(.2,.8,.25,1)',
          animation: awaiting ? 'mer-tour-pulse 1.6s ease-in-out infinite' : undefined,
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
            border: '2px solid var(--color-state-proposal)',
            animation: 'mer-tour-ping .5s ease-out',
          }} />
        )}
      </div>

      {/* Narration, pinned bottom-centre so it never covers the timeline column
          or the right panel the beats point at. */}
      {caption && centered && (
        <div className="absolute inset-0 flex items-center justify-center p-10"
          style={{ background: 'rgba(20,16,40,0.45)', backdropFilter: 'blur(2px)', pointerEvents: 'auto' }}>
          <div className="mer-pop text-center" style={{
            maxWidth: 520, padding: '30px 34px 26px', borderRadius: 20,
            background: 'var(--t-card)',
            border: '0.5px solid var(--t-card-border)',
            boxShadow: 'var(--mt-modal-shadow)',
          }}>
            {/* No eyebrow label. The centred card is used for the closing beat,
                where a "Getting started" tag contradicts the sentence under it —
                and the opening is now the full-screen title sequence, which
                frames the tour far better than a two-word tag ever did. */}
            <p style={{ fontSize: 17, lineHeight: 1.45, color: 'var(--t-title)', textWrap: 'pretty' }}>
              {caption}
            </p>
            {choices && choices.length > 0 && (
              <div className="flex gap-2 mt-5 justify-center flex-wrap">
                {choices.map((c, i) => <ChoiceButton key={c.value} choice={c} primary={i === 0} onChoose={onChoose} />)}
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
      {caption && !centered && ring && (
        <AnchoredCaption rect={ring}>
          <p style={{ fontSize: 13.5, lineHeight: 1.45, color: 'var(--t-title)', textWrap: 'pretty' }}>
            {caption}
          </p>
          {choices && choices.length > 0 && (
            <div className="flex gap-2 mt-3 flex-wrap">
              {choices.map((c, i) => <ChoiceButton key={c.value} choice={c} primary={i === 0} onChoose={onChoose} />)}
            </div>
          )}
        </AnchoredCaption>
      )}

      {caption && !centered && !ring && (
        <div className="absolute left-1/2 mer-pop" style={{
          bottom: 26, transform: 'translateX(-50%)', pointerEvents: 'auto',
          maxWidth: 620,
          padding: '13px 18px', borderRadius: 15,
          background: 'var(--t-card)',
          border: '0.5px solid var(--t-card-border)',
          boxShadow: 'var(--pop-shadow)',
        }}>
          <p style={{ fontSize: 13.5, lineHeight: 1.45, color: 'var(--t-title)', textWrap: 'pretty' }}>
            {caption}
          </p>
          {choices && choices.length > 0 && (
            <div className="flex gap-2 mt-3 flex-wrap">
              {choices.map((c, i) => <ChoiceButton key={c.value} choice={c} primary={i === 0} onChoose={onChoose} />)}
            </div>
          )}
        </div>
      )}

      {/* Skip — its own control, NOT inside the caption bubble.
          It used to live in there, which meant it vanished on any beat with an
          empty caption and moved horizontally as the caption's width changed.
          An escape hatch that relocates, or disappears exactly when a confused
          user reaches for it, is not an escape hatch. Fixed top-right, present
          for the entire run. */}
      <button onClick={onSkip} className="absolute" style={{
        top: 14, right: 16, pointerEvents: 'auto',
        fontSize: 12, color: 'var(--t-muted)', cursor: 'pointer',
        padding: '6px 13px', borderRadius: 99,
        background: 'var(--t-card)',
        border: '0.5px solid var(--t-card-border)',
        boxShadow: 'var(--pop-shadow)',
      }}>Skip tour</button>

      <style>{`
        @keyframes mer-tour-pulse {
          0%,100% { box-shadow: 0 0 0 4px color-mix(in srgb, var(--color-state-proposal) 22%, transparent); }
          50%     { box-shadow: 0 0 0 9px color-mix(in srgb, var(--color-state-proposal) 8%, transparent); }
        }
        @keyframes mer-tour-dim { from { opacity: 0 } to { opacity: 1 } }
        @keyframes mer-tour-ping {
          from { transform: scale(.6); opacity: .9 }
          to   { transform: scale(1.5); opacity: 0 }
        }
      `}</style>
    </div>
  )
}

/** A caption bubble parked against `rect` — ABOVE it by preference, below when
 *  there is not enough room above, and clamped so it never runs off either edge.
 *
 *  Above is the default because these captions are instructions for the thing
 *  underneath: read the line, then act on what it is sitting on. Below puts the
 *  words in the path of whatever the control opens or fills, and on a field it
 *  is where the caret and any validation text already are. */
function AnchoredCaption({ rect, children }: { rect: DOMRect; children: React.ReactNode }) {
  const W = 330
  const GAP = 16
  const vw = typeof window === 'undefined' ? 1200 : window.innerWidth
  const vh = typeof window === 'undefined' ? 800 : window.innerHeight
  // 130px is a generous guess at the tallest this bubble gets (three lines plus
  // a row of choices).
  const above = rect.top - GAP - 130 > 0
  const left = Math.min(Math.max(12, rect.left + rect.width / 2 - W / 2), vw - W - 12)
  return (
    <div className="absolute mer-pop" style={{
      left, width: W,
      ...(above ? { bottom: vh - rect.top + GAP } : { top: rect.bottom + GAP }),
      pointerEvents: 'auto',
      padding: '13px 16px', borderRadius: 15,
      background: 'var(--t-card)',
      border: '0.5px solid var(--t-card-border)',
      boxShadow: 'var(--mt-modal-shadow)',
      transition: 'top .28s cubic-bezier(.2,.8,.25,1), bottom .28s cubic-bezier(.2,.8,.25,1), left .28s cubic-bezier(.2,.8,.25,1)',
    }}>{children}</div>
  )
}

/** The blur-everything-else layer, as four panels fenced around `rect` rather
 *  than one masked full-screen pane.
 *
 *  A single pane with an SVG-masked hole is the tidier construction, but
 *  `backdrop-filter` under a mask is exactly the combination WebKit has been
 *  unreliable about, and this ships inside a WKWebView on macOS. Four plain
 *  rectangles are boring and render identically everywhere. The hole's corners
 *  come out square; the spotlight ring drawn on top is rounded and reads as the
 *  frame, so it does not show.
 *
 *  These DO take pointer events, which is half the point — while the tour is
 *  waiting on one specific click, a stray click into the blurred area should
 *  land on nothing rather than on some control the user cannot see clearly. */
function DimCutout({ rect }: { rect: DOMRect }) {
  const PAD = 8
  const l = Math.max(0, rect.left - PAD)
  const t = Math.max(0, rect.top - PAD)
  const r = rect.right + PAD
  const b = rect.bottom + PAD
  const pane: React.CSSProperties = {
    position: 'absolute',
    pointerEvents: 'auto',
    background: 'color-mix(in srgb, var(--win-bg) 55%, transparent)',
    backdropFilter: 'blur(5px)',
    WebkitBackdropFilter: 'blur(5px)',
    animation: 'mer-tour-dim .45s ease both',
  }
  return (
    <>
      <div style={{ ...pane, left: 0, top: 0, right: 0, height: t }} />
      <div style={{ ...pane, left: 0, top: b, right: 0, bottom: 0 }} />
      <div style={{ ...pane, left: 0, top: t, width: l, height: b - t }} />
      <div style={{ ...pane, left: r, top: t, right: 0, height: b - t }} />
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
  return (
    <button onClick={() => onChoose(choice.value)}
      className="text-left mt-card-hover"
      style={{
        padding: '9px 16px', borderRadius: 11, cursor: 'pointer',
        background: primary ? 'var(--color-state-proposal)' : 'var(--t-box)',
        color: primary ? '#fff' : 'var(--t-title)',
        border: primary ? 'none' : '1px solid var(--t-card-border)',
      }}>
      <span style={{ display: 'block', fontSize: 13, fontWeight: 700 }}>{choice.label}</span>
      {choice.hint && (
        <span style={{
          display: 'block', fontSize: 11.5, marginTop: 1,
          color: primary ? 'rgba(255,255,255,.82)' : 'var(--t-muted)',
        }}>{choice.hint}</span>
      )}
    </button>
  )
}
