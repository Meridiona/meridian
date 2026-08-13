//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The AI-provider picker — ONE component, mounted in both the setup wizard's Intelligence step
// and the dashboard's Settings, so the two surfaces can never drift. The provider list lives in
// `@/lib/llm-providers`.
//
// Two levels, on purpose (this replaced a flat grid of cards each carrying every control at
// once, which read as busy and buried the one decision):
//
//   GATE     — (only when `gate` is set) one question: do you already pay for a coding agent?
//              Answering routes to one of the two screens below and nothing else is shown, so
//              neither answer has to scan past options that cannot apply to it.
//   CHOOSER  — the three coding-agent tiles plus one "bring your own API key" tile, laid out
//              as a complete rectangle (3 across gated, 2 x 2 in Settings) - the fourth being
//              Groq, the free path. Each is a logo, a
//              name, a kind label, and — only when there is something to say — one status
//              pill. Just the choice, nothing else.
//   DETAIL   — click a tile and you get its own screen: install the CLI if missing, confirm the
//              sign-in if it's there, and make it the default. One
//              provider, one next action.
//
// It deliberately does NOT save. The wizard writes each pick straight through; Settings commits
// through its own save(). The owner does the write and this stays a controlled input.

import { BackLink } from '@/components/ui/BackLink'
import { useCallback, useState } from 'react'
import {
  CHOOSER_PROVIDER_IDS, GROQ, LLM_RANK_FREE, LLM_RANK_SUBSCRIPTION, LLM_RECOMMENDED_NOTE,
  llmProvider, type LlmProviderId, type LlmRank,
} from '@/lib/llm-providers'
import type {
  InstallOutcome, ProviderStatus, ProviderTestOutcome, ProviderTestResult,
} from '@/lib/api-types'
import { GroqLogo, ProviderLogo } from '@/components/LlmProviderLogos'
import LlmProviderDetail, { phaseFor, type Phase } from '@/components/LlmProviderDetail'
import LlmProviderGate, { Badge } from '@/components/LlmProviderGate'
import GroqSetup from '@/components/GroqSetup'

// Re-exported so the existing `from '@/components/LlmProviderPicker'` imports
// keep resolving. The definitions now live in api-types.ts, per the repo rule
// that Rust-mirrored invoke contracts live in one place.
export type {
  InstallOutcome, ProviderStatus, ProviderTestOutcome, ProviderTestResult,
}
import { CustomProviderCard, useCustomProviders } from '@/components/CustomProviders'


export { useLlmProviderDetection } from '@/components/useLlmProviderDetection'

/** Phases in which a SELECTED tile must not read as connected.
 *
 *  Deliberately the same two the old inline ternary raised a warning for, so this centralises
 *  the rule without changing what anyone sees:
 *
 *  * `rate_limited` is absent because a throttled provider IS working - it is signed in and
 *    answering, just pacing - which is exactly how the Rust classifier scores it
 *    (`classify_provider_health` keeps `ok: true` and sets `rate_limited`), and therefore how
 *    the dashboard banner reads it. A tile that went grey here would contradict the banner.
 *  * `ready_untested` / `unknown` are absent because "we have not checked" is not evidence of
 *    a problem. Greying every never-tested provider would flag the most ordinary state there
 *    is. */
const TILE_NOT_LIVE: Phase['kind'][] = ['not_installed', 'failed']

/** What a tile says about itself, given its phase: the failure text when something is wrong,
 *  otherwise the provider's own blurb.
 *
 *  `selected` only changes the wording of the not-installed case, where "you picked this and
 *  it is not here" is a materially different sentence from "this is not here". */
function tileMessage(phase: Phase, selected: boolean, subtitle?: string): string | undefined {
  switch (phase.kind) {
    case 'not_installed':
      return selected
        ? "Selected, but the CLI isn't installed on this machine."
        : 'Not installed on this machine.'
    case 'failed':
    case 'rate_limited':
      return phase.message
    default:
      return subtitle
  }
}

/** One tile in the top-level chooser: logo + name + a group label + at most one status pill.
 *
 *  ── THE CONNECTED VERDICT IS NOT DECIDED HERE ────────────────────────────────────────────
 *  The tile takes a `Phase` from [`phaseFor`] - the same function the detail screen and the
 *  Settings lock use, itself a mirror of Rust's `classify_provider_health`, which is what
 *  `get_health.llm_provider_ok` (the dashboard banner, the composer, the worklog dialog) is
 *  computed from. It used to take a hand-rolled `warning` prop built by an inline ternary in
 *  the grid below, which was a FOURTH implementation of the same ladder and disagreed with
 *  the other three in two ways that reached the screen: it had no notion of `rate_limited`
 *  (so a throttled provider showed a confident green tile while the banner said it was
 *  catching up), and the Groq tile passed it nothing at all - so a custom endpoint was green
 *  and IN USE purely because it was selected, and could not be told it had failed.
 *
 *  ── ONE tile is ever coloured, and only when something WORKS ─────────────────────────────
 *  Green means "this is yours and it is answering" - selected AND not in a broken phase.
 *  Everything else is plain, its message in muted grey.
 *
 *  Two earlier versions failed in the same direction. Colouring on `selected` alone put a
 *  green glow around a tile whose own badge said ERROR - and `selected` stays true when a
 *  provider breaks, so that is what a user saw the moment one stopped working. Colouring
 *  problems amber then turned the grid orange in the two most ordinary states there are: a
 *  fresh install with no CLIs, and a signed-out provider. Neither is an alarm, and if nothing
 *  is green that already says nothing is connected. */
function ChooserTile({ id, name, rank, selected, subtitle, phase, action, logo, onOpen }: {
  id: LlmProviderId; name: string; selected: boolean
  /** How this path ranks - the SAME pair of badges the gate showed for it. */
  rank: LlmRank
  subtitle?: string
  /** Where this provider stands, from the shared [`phaseFor`]. Never re-derived here. */
  phase: Phase
  /** A call to action pinned to the bottom of the tile - set when nothing on the grid is
   *  selected, so the row reads as a question rather than as a settled state. */
  action?: string
  /** Overrides the mark for a tile whose id is not an `LlmProviderId` (the Groq tile rides
   *  the `custom` id, so `ProviderLogo` would draw the generic API-key glyph for it). */
  logo?: React.ReactNode
  onOpen: () => void
}) {
  const live = selected && !TILE_NOT_LIVE.includes(phase.kind)
  const message = tileMessage(phase, selected, subtitle)

  return (
    <button onClick={onOpen} className="flex flex-col text-left h-full"
      style={{
        gap: 10, padding: '15px 16px', borderRadius: 13, cursor: 'pointer',
        background: live ? 'color-mix(in srgb, var(--color-state-approved) 9%, var(--t-card))' : 'var(--t-card)',
        border: `1.5px solid ${live ? 'var(--color-state-approved)' : 'var(--t-ctrl-border)'}`,
        boxShadow: live
          ? '0 0 0 3px color-mix(in srgb, var(--color-state-approved) 13%, transparent)'
          : '0 1px 2px rgba(0,0,0,.04)',
        transition: 'border-color 260ms ease, box-shadow 260ms ease, background 260ms ease',
      }}>
      <div className="flex items-start justify-between" style={{ gap: 10 }}>
        <span className="flex items-center justify-center shrink-0" style={{
          width: 38, height: 38, borderRadius: 10,
          background: live ? 'color-mix(in srgb, var(--color-state-approved) 13%, transparent)' : 'var(--t-box)',
          border: '1px solid var(--t-ctrl-border)',
        }}>
          {logo ?? <ProviderLogo id={id} size={21} />}
        </span>
        {/* Only on the chosen tile, and it never claims more than it knows. NOT CONNECTED
            rather than SELECTED: "selected" is a fact about settings.json that a user
            reasonably reads as a fact about the app. Grey - signed out is a state, not an
            alarm. */}
        {selected && (
          <span className="font-mono" style={{
            fontSize: 9, fontWeight: 700, letterSpacing: '.09em',
            color: live ? 'var(--color-state-approved)' : 'var(--t-faint)',
            border: `1px solid ${live ? 'var(--color-state-approved)' : 'var(--t-ctrl-border)'}`,
            background: live ? 'color-mix(in srgb, var(--color-state-approved) 10%, transparent)' : 'transparent',
            borderRadius: 4, padding: '2px 5px', whiteSpace: 'nowrap',
          }}>{live ? 'IN USE' : 'NOT CONNECTED'}</span>
        )}
      </div>
      <div className="flex flex-col" style={{ gap: 6 }}>
        <span style={{ fontSize: 14.5, fontWeight: 600, color: 'var(--t-title)' }}>{name}</span>
        {/* The ranking, carried across from the gate verbatim. Filled = the rank, outlined =
            what you get. Every tile has both, so the pair reads as a comparison rather than
            as a flag on one option; and because the difference is FILL rather than hue, it
            can sit on a screen where colour already means "connected" without colliding. */}
        <div className="flex items-center flex-wrap" style={{ gap: 5 }}>
          <Badge filled>{rank.badge}</Badge>
          <Badge>{rank.note}</Badge>
        </div>
        {/* Muted, like any other subtitle. This is what the tile has to say about itself,
            not a warning the user must act on before they have even opened it. */}
        <span style={{ fontSize: 11, lineHeight: 1.45, color: 'var(--t-muted)' }}>
          {message}
        </span>
      </div>
      {action && (
        <span className="flex items-center mt-auto" style={{
          gap: 4, fontSize: 11.5, fontWeight: 700, color: 'var(--t-accent)', paddingTop: 2,
        }}>
          {action}
          <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor"
            strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M6 4l4 4-4 4" />
          </svg>
        </span>
      )}
    </button>
  )
}

export interface LlmProviderPickerProps {
  value: LlmProviderId
  /** Which custom endpoint, when `value` is 'custom'. */
  selectedCustomId?: string | null
  /** The owner persists BOTH fields together (`llm_provider` + `llm_provider_custom_id`). May
   *  return a promise the detail view awaits to show "Switching…"/failure. */
  onChange: (id: LlmProviderId, customId?: string) => void | Promise<void>
  status: Record<string, ProviderStatus>
  scanning: boolean
  testingIds: Set<string>
  installingIds: Set<string>
  signingIds: Set<string>
  testOne: (id: string) => void
  install: (id: string) => Promise<InstallOutcome>
  signIn: (id: string) => Promise<InstallOutcome>
  rescan: () => void
  /** Open on the subscription question instead of the chooser.
   *
   *  Set by the surfaces where the user is being SET UP - the first-run wizard, and the
   *  walkthrough's "Meridian needs an AI engine" path. Settings opened normally leaves it
   *  off: someone who came to change their model is not asking to be interviewed about
   *  their subscriptions again, and re-asking would bury the control they came for. */
  gate?: boolean
}

export default function LlmProviderPicker({
  value, selectedCustomId, onChange, status, scanning, testingIds, installingIds, signingIds,
  testOne, install, signIn, rescan, gate = false,
}: LlmProviderPickerProps) {
  // Which provider's detail is open, or null for the chooser. `'custom'` opens the Groq flow.
  const [openId, setOpenId] = useState<LlmProviderId | null>(null)
  // The gate's answer. `null` = unanswered, which is the opening screen only when gated.
  const [gateAnswer, setGateAnswer] = useState<'subscription' | 'free' | null>(
    gate ? null : 'subscription',
  )
  // A provider the user picked DURING this gated flow. Under the gate nothing is shown as
  // selected on arrival (`value` falls back to Claude whether or not it works), but once
  // they have actually chosen one, coming back to the grid must show which - otherwise the
  // screen looks exactly as it did before they did anything.
  const [committedId, setCommittedId] = useState<LlmProviderId | null>(null)
  const commitAndMark = useCallback(async (id: LlmProviderId, customId?: string) => {
    await onChange(id, customId)
    setCommittedId(id)
  }, [onChange])
  const custom = useCustomProviders()
  // Where the cloud endpoint stands, computed ONCE and shared by the Groq tile and the
  // endpoint screen behind it. Two call sites deriving it separately is how they came to
  // disagree - the tile said NOT CONNECTED while the card behind it said "In use".
  const customPhase = phaseFor(false, false, false, status.custom, status.custom?.last_test ?? null)

  if (gateAnswer === null) {
    // Clearing `openId` with the answer: re-answering must land on that answer's own first
    // screen, never on a detail view left open from the other branch.
    return <LlmProviderGate onAnswer={(a) => { setOpenId(null); setGateAnswer(a) }} />
  }

  // "No subscription" goes STRAIGHT to Groq - no intermediate list of one. The gate already
  // took the only decision there was; showing a chooser containing a single tile would be
  // asking the user to confirm an answer they just gave.
  if (gateAnswer === 'free') {
    return (
      <GroqSetup
        onBack={() => setGateAnswer(null)}
        onAdd={custom.add}
        onPick={(id) => commitAndMark('custom', id)}
      />
    )
  }

  // A provider the user has selected but that isn't one of the three recommended tiles (a
  // legacy Copilot pick). We don't render a Copilot card, but we mustn't strand them either.
  const usingHidden = value !== 'custom' && !CHOOSER_PROVIDER_IDS.includes(value)

  if (openId && openId !== 'custom') {
    const p = llmProvider(openId)
    return (
      <LlmProviderDetail
        p={p}
        probed={status[openId]}
        testing={testingIds.has(openId)}
        installing={installingIds.has(openId)}
        signing={signingIds.has(openId)}
        selected={value === openId}
        onBack={() => setOpenId(null)}
        onInstall={() => install(openId)}
        onSignIn={() => signIn(openId)}
        onTest={() => testOne(openId)}
        onUse={async () => { await commitAndMark(openId) }}
      />
    )
  }

  if (openId === 'custom') {
    // The fourth tile is GROQ, so it opens the Groq walkthrough - the same screen the gate's
    // "no subscription" answer lands on, not a second route to the same place. Once an
    // endpoint IS on file the registry view takes over: that is where a key is swapped,
    // re-probed or removed, and the walkthrough has nothing left to walk through.
    if (!custom.loading && custom.providers.length === 0) {
      return (
        <GroqSetup
          onBack={() => setOpenId(null)}
          onAdd={custom.add}
          onPick={(id) => commitAndMark('custom', id)}
        />
      )
    }
    return (
      <CustomDetail
        value={value}
        selectedCustomId={selectedCustomId ?? null}
        // The SAME verdict the tile the user just clicked was showing. Passed down rather
        // than recomputed so the two screens cannot disagree about one provider.
        live={!TILE_NOT_LIVE.includes(customPhase.kind)}
        // The recorded reason, so the card reports rather than guesses.
        statusDetail={'message' in customPhase ? customPhase.message : undefined}
        custom={custom}
        onBack={() => setOpenId(null)}
        onPick={(id) => commitAndMark('custom', id)}
        // What actually answers "is it connected". A schema probe measures what the endpoint
        // CAN do; only a real connectivity test writes the verdict every other surface reads
        // (`provider_test_cache.json` → `llm_provider_ok` → the banner, the composer, the
        // tiles). Without this the card was a dead end: an endpoint marked Not connected had
        // Re-test and Change key, and neither could clear the verdict - Re-test measured
        // schemas and never touched it. `rescan` afterwards so the row reappears in `status`
        // even when there was no cached result to update in place.
        onVerify={async () => { await testOne('custom'); rescan() }}
      />
    )
  }

  return (
    <div className="flex flex-col" style={{ gap: 12 }}>
      {/* Only when the gate asked. Answering "yes" is a claim about what the user owns, and
          a claim they may want to take back once they see the three names - so the way back
          is a visible control rather than a modal dismissal. */}
      {gate && (
        <BackLink onClick={() => setGateAnswer(null)}>I don&apos;t have one</BackLink>
      )}
      {/* Column count pinned to the tile count, not auto-filled, so the grid is always a
          complete rectangle: 3 across under the gate, 2 x 2 in Settings (those three plus
          Groq). Auto-fill gave four tiles a 3 + 1 layout - "and one afterthought" - with the
          last card stretched to a different shape from its siblings. */}
      <div style={{
        display: 'grid', gap: 10,
        gridTemplateColumns: `repeat(${gate ? 3 : 2}, minmax(0, 1fr))`,
      }}>
        {CHOOSER_PROVIDER_IDS.map((id) => {
          const p = llmProvider(id)
          const probed = status[id]
          // UNDER THE GATE, NOTHING IS SELECTED. `value` is never empty - it falls back to
          // Claude (LlmProvider::default()), which is a sensible default for the RESOLVER
          // but a lie on this screen: the user arrived here precisely because no provider
          // works, and a highlighted Claude tile tells them a choice has already been made
          // and there is nothing to do. It also puts the ring on whichever provider is
          // stored, which under the gate is at best arbitrary and at worst the one that
          // just failed.
          const isSelected = (!gate || committedId === id) && value === id
          return (
            <ChooserTile
              key={id}
              id={id}
              name={p.name}
              rank={LLM_RANK_SUBSCRIPTION}
              selected={isSelected}
              // What it runs on, in plain words - the tile used to carry a mono label saying
              // USES YOUR SUBSCRIPTION, which the blurb says better and at a readable size.
              subtitle={p.blurb}
              // The shared ladder, not a local one. The in-flight flags are false here: a tile
              // is a summary, and the spinners for install/sign-in/test belong to the detail
              // screen that owns those actions.
              phase={phaseFor(false, false, false, probed, probed?.last_test ?? null)}
              // Under the gate every tile carries the same instruction, because none of
              // them is selected and a grid of three equal cards with no call to action
              // does not read as a question waiting on an answer.
              action={gate ? 'Choose' : undefined}
              onOpen={() => setOpenId(id)}
            />
          )
        })}
        {/* The free option, NAMED. "Bring your own API key / ADVANCED" described the
            plumbing rather than the offer: someone with no subscription - the exact person
            this tile is for - could not tell it was the free path, and "advanced" warns them
            off it. Groq is the only preset left, so the tile is Groq, badged FREE. Hidden
            under the gate, which already asked this exact question. */}
        {!gate && (
          <ChooserTile
            id="custom"
            name={GROQ.name}
            rank={LLM_RANK_FREE}
            selected={value === 'custom'}
            subtitle={GROQ.blurb}
            // The SAME ladder as the three CLI tiles. Without it this tile was green and said
            // IN USE off `value === 'custom'` alone - it had no way to disagree, so a provider
            // that had just failed a test (or been disconnected from the dev panel) still read
            // as connected, while the dashboard banner two screens away said the opposite.
            // `detect_llm_providers` carries the cached verdict for `custom` through for
            // exactly this.
            phase={customPhase}
            logo={<GroqLogo size={21} />}
            onOpen={() => setOpenId('custom')}
          />
        )}
      </div>

      <p style={{ fontSize: 13.5, lineHeight: 1.5, fontWeight: 500, color: 'var(--t-muted)' }}>
        {LLM_RECOMMENDED_NOTE}
      </p>

      {/* A legacy Copilot user isn't stranded: tell them what they're on and let them switch. */}
      {usingHidden && (
        <p style={{ fontSize: 11, lineHeight: 1.45, color: 'var(--color-state-pending)' }}>
          You&apos;re currently using {llmProvider(value).name}. Pick one of the recommended providers
          above to switch.
        </p>
      )}

      {/* Re-probe for a CLI installed in a terminal while this was open. Hidden under the
          gate: a first-time user has no model for what "rescan" means, and the detail view
          installs the CLI for them anyway. Detection still re-runs after an install or a
          sign-in regardless. */}
      {!gate && (
        <button onClick={rescan} disabled={scanning} className="self-start flex items-center"
          style={{
            gap: 7, fontSize: 11.5, fontWeight: 600, color: 'var(--t-muted)',
            background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
            borderRadius: 8, padding: '6px 11px',
            cursor: scanning ? 'default' : 'pointer',
          }}>
          {/* A real spinner, and a button that looks like one. As a bare underlined link
              with only its label changing, a rescan that resolves in a few milliseconds gave
              no evidence it had run at all - the user pressed it, nothing moved, and the
              only reasonable conclusion was that it is broken. `detect` also holds a floor
              on how briefly this can be true, so the answer is always visible. */}
          {scanning ? (
            <span className="inline-block shrink-0" style={{
              width: 12, height: 12, borderRadius: 99,
              border: '2px solid var(--t-ctrl-border)', borderTopColor: 'var(--t-accent)',
              animation: 'spin 0.7s linear infinite',
            }} />
          ) : (
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor"
              strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M13.6 6.5A5.8 5.8 0 1 0 14 8.8" /><path d="M13.9 3v3.6h-3.6" />
            </svg>
          )}
          {scanning ? 'Checking what’s installed…' : 'Installed a CLI yourself? Rescan'}
        </button>
      )}
    </div>
  )
}

/** The endpoint registry — what the Groq tile opens once a key is already on file.
 *
 *  ONE HEADING, ONE ROW PER ENDPOINT, AND NOTHING ELSE. What used to be here: a title, a
 *  two-clause paragraph about billing, a grid of 232px tiles each carrying a status pill, a
 *  model, a rung label, a quota notice and two shouting buttons, and a dashed "+ Add a custom
 *  endpoint" tile. All of it described a configuration surface that no longer exists - there
 *  is exactly one way to get an endpoint (<GroqSetup>) and, in practice, exactly one endpoint.
 *
 *  The billing paragraph went with the add path it warned about: "any other OpenAI-compatible
 *  endpoint bills you per call" is advice for a decision this screen no longer offers, and it
 *  was the loudest text on a screen whose actual subject is free. */
function CustomDetail({ value, selectedCustomId, live, statusDetail, custom, onBack, onPick, onVerify }: {
  value: LlmProviderId
  selectedCustomId: string | null
  /** Whether the shared phase ladder says this provider is answering - see `ChooserTile`. */
  live: boolean
  /** Why not, verbatim from the recorded test. Undefined when nothing is wrong. */
  statusDetail?: string
  custom: ReturnType<typeof useCustomProviders>
  onBack: () => void
  onPick: (id: string) => void
  /** Run the real connectivity test and refresh what every surface reads. */
  onVerify: () => Promise<void>
}) {
  return (
    <div className="flex flex-col" style={{ gap: 14 }}>
      <BackLink onClick={onBack}>All providers</BackLink>

      <div className="flex flex-col" style={{ gap: 3 }}>
        <span style={{ fontSize: 18, fontWeight: 600, color: 'var(--t-title)', letterSpacing: '-.01em' }}>
          Your API key
        </span>
        {/* One line, and it says what the key DOES rather than what a different kind of key
            might cost. 13px: a subtitle under an 18px heading, not a footnote. */}
        <p style={{ fontSize: 13, lineHeight: 1.5, color: 'var(--t-muted)' }}>
          Meridian writes your summaries on this key. Nothing is charged.
        </p>
      </div>

      {/* A column, not a grid. One endpoint in a three-across grid rendered as a lone
          narrow tile with two thirds of the row empty. */}
      <div className="flex flex-col" style={{ gap: 8, maxWidth: 460 }}>
        {custom.providers.map((c) => (
          <CustomProviderCard
            key={c.id}
            p={c}
            picked={value === 'custom' && selectedCustomId === c.id}
            live={live && value === 'custom' && selectedCustomId === c.id}
            statusDetail={statusDetail}
            probing={custom.probingIds.has(c.id)}
            onPick={() => onPick(c.id)}
            // Both halves, in the order that matters: measure what the endpoint supports,
            // then ask whether it actually answers. The second is what clears a stale
            // "Not connected".
            onProbe={async (hard) => { await custom.probe(c.id, hard); await onVerify() }}
            onReplaceKey={async (apiKey) => {
              await custom.replaceKey(c.id, apiKey)
              await onVerify()
            }}
          />
        ))}
      </div>
    </div>
  )
}
