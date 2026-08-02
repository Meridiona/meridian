//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The beat between "Draft with AI" and the provider picker.
//
// Without it, pressing Draft on a machine with no provider connected swapped the
// whole screen for Settings in one frame. Nothing had visibly caused it: the
// user pressed a button on a form and landed in a settings pane, which reads as
// a misclick or a bug rather than an answer. The fix is not a slower animation —
// it is naming the reason before moving, and letting the move be their click.
//
// One line and one button, deliberately. Everything else that could be said here
// (which features need a model, whose machine it runs on, that nothing leaves
// the device) belongs on the provider screen, where the user is choosing between
// providers and the answer is load-bearing. Said HERE it is a paragraph standing
// between someone and the button they already know they want.
//
// No dismiss. There is nothing to dismiss: this sits inline under the note field,
// the title and description below it stay editable throughout, and "Add to today"
// never depended on a model. Someone who does not want an AI provider simply
// keeps typing — a "Not now" button only added a second way to do nothing, and
// spent the card's one clear call to action arguing with itself.
//
// IT ARRIVES SLOWLY, on purpose. This card is the answer to a button the user
// just pressed, and an answer that snaps in under the cursor in one frame reads
// as the layout jumping rather than as a reply. It fades and rises over ~600ms
// and scrolls itself into view, so the eye is carried to it instead of having to
// find it — the same reason the button above holds a "Checking…" state first.
//
// # Who calls this
// [`TaskComposer`], under the note field, when `get_health`'s `llm_provider_ok`
// is false.

import { useEffect, useRef, useState } from 'react'

/** How long the entrance takes. Long for a UI transition, deliberately: it is
 *  covering a change of subject, not decorating a hover. */
const REVEAL_MS = 620

/** @param onConnect Open the provider picker (Settings → Intelligence). */
export function AiEngineNotice({ onConnect }: { onConnect: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  const [shown, setShown] = useState(false)
  useEffect(() => {
    // Next frame, so the browser has the "before" state to transition FROM -
    // setting both in the same commit animates nothing.
    const raf = requestAnimationFrame(() => setShown(true))
    const t = setTimeout(() => ref.current?.scrollIntoView({ behavior: 'smooth', block: 'center' }), 140)
    return () => { cancelAnimationFrame(raf); clearTimeout(t) }
  }, [])

  return (
    <div ref={ref} className="rounded-xl p-4 mt-2 flex items-center gap-3" style={{
      background: 'color-mix(in srgb, var(--color-state-proposal) 8%, transparent)',
      border: '1px solid color-mix(in srgb, var(--color-state-proposal) 28%, transparent)',
      opacity: shown ? 1 : 0,
      transform: shown ? 'none' : 'translateY(10px)',
      transition: `opacity ${REVEAL_MS}ms ease, transform ${REVEAL_MS}ms cubic-bezier(.2,.7,.3,1)`,
    }}>
      <span className="inline-flex items-center justify-center rounded-full shrink-0 text-[13px]"
        style={{
          width: 26, height: 26,
          background: 'color-mix(in srgb, var(--color-state-proposal) 20%, transparent)',
        }}>✨</span>
      <p className="mt-card-title flex-1 min-w-0" style={{ color: 'var(--t-title)' }}>
        Meridian needs an AI engine
      </p>
      {/* data-tour: the first-run walkthrough points here when the draft beat
          lands on a machine with no provider. See ui/components/tutorial/script.ts. */}
      <button data-tour="ai-connect" onClick={onConnect}
        className="mt-body-sm px-3.5 py-2 rounded-lg shrink-0"
        style={{
          fontWeight: 700, color: '#fff',
          background: 'var(--color-state-proposal)',
          boxShadow: '0 8px 22px -10px var(--color-state-proposal)',
        }}>
        Connect a provider
      </button>
    </div>
  )
}
