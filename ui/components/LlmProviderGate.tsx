//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The question that comes BEFORE any provider is shown: do you already pay for a coding
// agent, or not?
//
// The old first screen was a grid of four tiles. Three of them are inert without a paid
// subscription and the fourth said "advanced", so the screen quietly demanded that the user
// already know which group they were in - at the exact moment they had asked for nothing
// more than a drafted task. Asking outright costs one click and lets each answer get a
// screen built for it: three CLIs, or a three-step free key.
//
// THE RECOMMENDATION IS TWO BADGES AND NOTHING ELSE. Both answers work, and one of them is
// free; arguing for the paid one in prose reads as a sales pitch on a screen the user did
// not come here for. But saying nothing would leave someone who HAS a Claude plan on the
// weaker engine for want of a hint. A badge is the smallest thing that states a preference
// without pressing it - see LLM_GATE_CHOICES.
//
// AND NOTHING HERE IS COLOURED. The two cards used to carry a green border and a violet one,
// which broke the rule the rest of this flow runs on: in Meridian green means "connected and
// working" and violet means "AI wrote this". Neither is true of an unanswered question, so
// the colours were saying something about state at the one moment there is no state - and
// they ranked the two answers by hue on top of it, which is exactly what the badges are here
// to do instead. The primary badge is distinguished by being FILLED, not by being a
// different colour: emphasis without a second meaning.
//
// # Who calls this
// [`LlmProviderPicker`], as its first screen when `gate` is set - the first-run wizard and
// the walkthrough's connect path. Settings opened normally skips straight to the chooser,
// because someone who came to change their model is not asking to be re-interviewed.

import {
  LLM_GATE_BODY, LLM_GATE_CHOICES, LLM_GATE_TITLE, type LlmGateChoice,
} from '@/lib/llm-providers'

/** @param onAnswer Given the picked answer's `value` - the caller routes on it. */
export default function LlmProviderGate({ onAnswer }: {
  onAnswer: (value: LlmGateChoice['value']) => void
}) {
  return (
    <div className="flex flex-col" style={{ gap: 15 }}>
      <div>
        <h2 style={{ fontSize: 17, fontWeight: 600, color: 'var(--t-title)', lineHeight: 1.3 }}>
          {LLM_GATE_TITLE}
        </h2>
        <p style={{ fontSize: 12, lineHeight: 1.5, color: 'var(--t-muted)', marginTop: 5, maxWidth: 520 }}>
          {LLM_GATE_BODY}
        </p>
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(240px, 1fr))', gap: 11 }}>
        {LLM_GATE_CHOICES.map((c) => (
          <GateCard key={c.value} choice={c} onPick={() => onAnswer(c.value)} />
        ))}
      </div>
    </div>
  )
}

/** One answer. Identical to its sibling in every respect a user can see except the words -
 *  same border, same weight, same size, no hue. That is what makes the badges the ranking. */
function GateCard({ choice, onPick }: { choice: LlmGateChoice; onPick: () => void }) {
  return (
    <button
      data-tour={`gate-${choice.value}`}
      onClick={onPick}
      className="flex flex-col text-left h-full mer-pop"
      style={{
        gap: 10, padding: '16px 16px', borderRadius: 13, cursor: 'pointer',
        background: 'var(--t-card)',
        border: '1px solid var(--t-ctrl-border)',
        boxShadow: '0 1px 2px rgba(0,0,0,.04)',
      }}>
      <div className="flex items-center flex-wrap" style={{ gap: 6 }}>
        <Badge filled>{choice.badge}</Badge>
        <Badge>{choice.note}</Badge>
      </div>
      <span style={{ fontSize: 15, fontWeight: 600, color: 'var(--t-title)' }}>{choice.label}</span>
      <span style={{ fontSize: 11.5, lineHeight: 1.45, color: 'var(--t-muted)' }}>{choice.detail}</span>
    </button>
  )
}

/** The primary badge and the secondary one differ by CONTRAST, not by colour and not by
 *  weight of ink: a soft tinted chip in full-strength text, against a bare outline in muted
 *  text. Both stay small and quiet.
 *
 *  Two things it deliberately is not. Not a hue - every colour in this app already means
 *  something (green connected, violet AI, amber attention) and a rank badge means none of
 *  them. And not a solid dark chip, which was the first attempt at "emphasis without
 *  colour": near-black blocks are the heaviest thing that can appear at this size, so two of
 *  them turned a quiet comparison into the loudest element on the screen.
 *
 *  Exported because the chooser grid shows the SAME pair on its tiles (see LLM_RANK_*): the
 *  gate ranks the two paths, then the grid shows one path's tiles - telling someone BEST
 *  ACCURACY on the question and then nothing on the tiles hands them a ranking and takes it
 *  away at the moment they act on it. One component, so the two cannot drift apart. */
export function Badge({ children, filled }: { children: React.ReactNode; filled?: boolean }) {
  return (
    <span className="font-mono" style={{
      fontSize: 8.5, fontWeight: 700, letterSpacing: '.09em',
      color: filled ? 'var(--t-title)' : 'var(--t-faint)',
      border: '1px solid var(--t-ctrl-border)',
      background: filled ? 'var(--t-box)' : 'transparent',
      borderRadius: 4, padding: '2px 6px', whiteSpace: 'nowrap',
    }}>{children}</span>
  )
}
