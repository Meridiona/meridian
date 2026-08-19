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
//   CHOOSER  — the three coding-agent tiles plus one tile PER no-subscription preset (Groq,
//              Ollama - see `CLOUD_PRESETS`), auto-wrapping rather than pinned to an exact
//              rectangle so a third preset fits without a layout edit. Each is a logo, a
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
  CHOOSER_PROVIDER_IDS, CLOUD_PRESETS, customVariantId, LLM_RANK_FREE, LLM_RANK_SUBSCRIPTION,
  LLM_RECOMMENDED_NOTE, llmProvider, presetForVendor, type CloudKeyPreset, type CustomProviderView,
  type LlmProviderId, type LlmRank,
} from '@/lib/llm-providers'
import type {
  InstallOutcome, ProviderStatus, ProviderTestOutcome, ProviderTestResult,
} from '@/lib/api-types'
import { CustomVendorLogo, ProviderLogo } from '@/components/LlmProviderLogos'
import LlmProviderDetail, { phaseFor, type Phase } from '@/components/LlmProviderDetail'
import LlmProviderGate, { Badge } from '@/components/LlmProviderGate'
import CloudKeySetup from '@/components/CloudKeySetup'

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
  /** Overrides the mark for a tile whose id is not an `LlmProviderId` (the free-key tile
   *  rides the `custom` id, so `ProviderLogo` would draw the generic API-key glyph for it -
   *  this is where its vendor logo, or the generic key glyph, comes from instead). */
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

/** One no-subscription preset's OWN tile in the top-level grid - Groq's, or Ollama's, never
 *  a tile shared between them.
 *
 *  Derives its own `selected`/`phase` from whichever of THIS vendor's rows (there is usually
 *  at most one) is the currently active provider, rather than from a single shared "custom"
 *  verdict. Reads `status[customVariantId(selectedRow.id)]`, NOT `status.custom` - the Rust
 *  side files the active endpoint's test/health under its OWN id (`provider_key`) precisely
 *  because the bare `"custom"` bucket is shared by every configured endpoint: reading it
 *  directly is what let a real user see Groq's stale rate-limit message rendered on Ollama's
 *  tile after switching to it. A not-currently-selected vendor's tile correctly shows
 *  "nothing to say" (the same untested-neutral phase an unselected CLI tile shows) rather
 *  than borrowing anything. */
function CloudPresetTile({ preset, value, selectedCustomId, status, custom, onOpen }: {
  preset: CloudKeyPreset
  value: LlmProviderId
  selectedCustomId: string | null
  status: Record<string, ProviderStatus>
  custom: ReturnType<typeof useCustomProviders>
  onOpen: () => void
}) {
  // `preset` is always a member of `CLOUD_PRESETS` here - the grid only ever builds one of
  // these tiles from `gridPresets`, which IS `CLOUD_PRESETS` (see its own comment on why a
  // discontinued vendor like Groq no longer gets a tile at all rather than a "blocked" one).
  const rows = custom.providers.filter((p) => p.vendor === preset.vendor)
  const selectedRow = rows.find((p) => p.id === selectedCustomId)
  const selected = value === 'custom' && !!selectedRow
  const activeStatus = selectedRow ? status[customVariantId(selectedRow.id)] : undefined
  const phase = selected
    ? phaseFor(false, false, false, activeStatus, activeStatus?.last_test ?? null)
    : phaseFor(false, false, false, undefined, null)
  return (
    <ChooserTile
      id="custom"
      // The row's own name (a user may have renamed it) once one exists; the preset's name
      // beforehand - which is exactly what this tile represents either way.
      name={selectedRow ? selectedRow.name : preset.name}
      rank={LLM_RANK_FREE}
      selected={selected}
      subtitle={selectedRow ? `Uses your ${selectedRow.name} key.` : preset.blurb}
      phase={phase}
      logo={<CustomVendorLogo vendor={preset.vendor} size={21} />}
      onOpen={onOpen}
    />
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
  // Which provider's detail is open, or null for the chooser. `'custom'` opens the
  // no-subscription flow for `openVendor` (that preset's registry view, or its setup
  // wizard directly if nothing is configured on it yet).
  const [openId, setOpenId] = useState<LlmProviderId | null>(null)
  // WHICH preset the `'custom'` tile that was clicked stands for - `'groq'` or `'ollama'`.
  // Groq and Ollama are each their OWN tile in the grid now (see the render below), so the
  // tile itself already answers "which vendor", unlike the old single collapsed "custom"
  // tile that had to guess or ask.
  const [openVendor, setOpenVendor] = useState<string | null>(null)
  // The gate's answer. `null` = unanswered, which is the opening screen only when gated.
  const [gateAnswer, setGateAnswer] = useState<'subscription' | 'free' | null>(
    gate ? null : 'subscription',
  )
  // A preset (Groq/Ollama) mid-setup - shared by the two places that can open
  // `<CloudKeySetup>` directly for one: the gate's "free" answer (picking between the two)
  // and "add another key" on an already-populated vendor's registry view. Never set from
  // the main grid's per-vendor tiles - those already know their vendor and skip straight to
  // whichever screen (wizard or registry) that vendor needs, via `openVendor` instead.
  const [pendingPreset, setPendingPreset] = useState<CloudKeyPreset | null>(null)
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

  if (gateAnswer === null) {
    // Clearing `openId` with the answer: re-answering must land on that answer's own first
    // screen, never on a detail view left open from the other branch.
    return <LlmProviderGate onAnswer={(a) => { setOpenId(null); setGateAnswer(a) }} />
  }

  // "No subscription" goes STRAIGHT to the one offered preset when there is only one - no
  // intermediate list of one, which is exactly asking the user to confirm an answer they
  // just gave (this is where Groq's removal from `CLOUD_PRESETS` lands: back to a single
  // offered preset, so the chooser screen this block used to always show would now show a
  // single Ollama tile for no reason). The chooser comes back on its own the moment a second
  // preset is offered again - nothing here assumes exactly one or exactly two.
  if (gateAnswer === 'free') {
    if (pendingPreset) {
      return (
        <CloudKeySetup
          preset={pendingPreset}
          onBack={() => setPendingPreset(null)}
          onAdd={custom.add}
          onPick={(id) => commitAndMark('custom', id)}
        />
      )
    }
    if (CLOUD_PRESETS.length === 1) {
      return (
        <CloudKeySetup
          preset={CLOUD_PRESETS[0]}
          onBack={() => setGateAnswer(null)}
          onAdd={custom.add}
          onPick={(id) => commitAndMark('custom', id)}
        />
      )
    }
    return <CloudPresetChooser onBack={() => setGateAnswer(null)} onPick={setPendingPreset} />
  }

  // A provider the user has selected but that isn't one of the three recommended tiles (a
  // legacy Copilot pick). We don't render a Copilot card, but we mustn't strand them either.
  const usingHidden = value !== 'custom' && !CHOOSER_PROVIDER_IDS.includes(value)
  // The cloud-endpoint equivalent of `usingHidden`: a discontinued vendor (Groq) no longer
  // gets a grid tile at all, so someone still ACTIVELY on it needs the same "you're not
  // stranded, here's what you're on and where to go" line - otherwise the screen just looks
  // empty where their provider used to be, with no explanation.
  const activeRow = custom.providers.find((p) => p.id === selectedCustomId)
  const usingHiddenCloud =
    value === 'custom' && !!activeRow && !CLOUD_PRESETS.some((p) => p.vendor === activeRow.vendor)

  // Every preset tile the grid shows: exactly the OFFERED presets (Ollama), full stop.
  //
  // This used to also union in every vendor the user already had a configured row for but
  // that had stopped being offered (Groq) - the same "don't strand a legacy pick" principle
  // `usingHidden` above still applies to CLI providers. That was deliberately reverted for
  // Groq specifically: a tile that can only ever say "discontinued, switch to Ollama" is
  // worse than no tile at all - it permanently announces a dead end to someone who has to
  // look at this screen. The real enforcement lives in Rust (`resolver::groq_blocked`
  // refuses every call, and `main.rs` raises a persistent notice while Groq is active) - it
  // does not depend on a grid tile existing. `usingHiddenCloud` below still tells anyone
  // ACTIVELY on Groq what they're on and where to go, so no one is silently stranded.
  const gridPresets = CLOUD_PRESETS

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

  if (openId === 'custom' && openVendor) {
    // `presetForVendor`, NOT `CLOUD_PRESETS.find` - this is a DISPLAY lookup, and a vendor
    // can have an open tile (and therefore reach this branch) without being currently
    // OFFERED - a retired preset like Groq, for a user who configured it before it was
    // retired. `CLOUD_PRESETS.find` would return undefined there and crash the screen the
    // moment someone clicked their own already-working endpoint.
    const preset = presetForVendor(openVendor)
    if (!preset) {
      // A vendor string this build has no display data for at all (a hand-edited
      // settings.json, or a future preset added to Rust before its frontend data
      // landed) - there is nothing sensible to render for it, so offer a way back
      // rather than crash on an undefined preset.
      return (
        <BackLink onClick={() => { setOpenId(null); setOpenVendor(null) }}>
          All providers
        </BackLink>
      )
    }
    const offered = CLOUD_PRESETS.some((p) => p.vendor === openVendor)
    const vendorRows = custom.providers.filter((p) => p.vendor === openVendor)
    // Mid-"add another key on this vendor": owns the screen until it finishes or backs out,
    // returning to this vendor's registry view rather than all the way to the grid.
    if (pendingPreset) {
      return (
        <CloudKeySetup
          preset={pendingPreset}
          onBack={() => setPendingPreset(null)}
          onAdd={custom.add}
          onPick={(id) => commitAndMark('custom', id)}
        />
      )
    }
    // This vendor's OWN tile was clicked, so it already answers "which preset" - straight to
    // its wizard with nothing configured yet, or straight to its (filtered) registry view
    // once something is. No intermediate chooser: there is nothing left to choose between.
    if (!custom.loading && vendorRows.length === 0) {
      return (
        <CloudKeySetup
          preset={preset}
          onBack={() => { setOpenId(null); setOpenVendor(null) }}
          onAdd={custom.add}
          onPick={(id) => commitAndMark('custom', id)}
        />
      )
    }
    // Whether the SELECTED row (if any) among this vendor's own rows is answering. Two call
    // sites deriving this separately is how they used to disagree - the tile said NOT
    // CONNECTED while the card behind it said "In use". Reads the row's OWN key
    // (`customVariantId`), not `status.custom` - see `CloudPresetTile`'s comment on why
    // that shared bucket is wrong the moment more than one endpoint exists.
    const selectedRow = vendorRows.find((p) => p.id === selectedCustomId)
    const activeStatus = selectedRow ? status[customVariantId(selectedRow.id)] : undefined
    const vendorPhase = selectedRow
      ? phaseFor(false, false, false, activeStatus, activeStatus?.last_test ?? null)
      : phaseFor(false, false, false, undefined, null)
    return (
      <CustomDetail
        preset={preset}
        providers={vendorRows}
        value={value}
        selectedCustomId={selectedCustomId ?? null}
        live={!TILE_NOT_LIVE.includes(vendorPhase.kind)}
        statusDetail={'message' in vendorPhase ? vendorPhase.message : undefined}
        custom={custom}
        onBack={() => { setOpenId(null); setOpenVendor(null) }}
        onPick={(id) => commitAndMark('custom', id)}
        // A second key on the SAME vendor - a personal key and a work key, say. Opens the
        // wizard for this exact preset again, never a chooser (the vendor is already fixed).
        // `undefined` (not a no-op handler) when the vendor isn't offered any more - Groq's
        // existing key stays fully manageable, but nothing invites a SECOND one once it's
        // been discontinued for new setups.
        onAddAnother={offered ? () => setPendingPreset(preset) : undefined}
        // What actually answers "is it connected". A schema probe measures what the endpoint
        // CAN do; only a real connectivity test writes the verdict every other surface reads
        // (`provider_test_cache.json` → `llm_provider_ok` → the banner, the composer, the
        // tiles). Without this the card was a dead end: an endpoint marked Not connected had
        // Re-test and Change key, and neither could clear the verdict - Re-test measured
        // schemas and never touched it. `rescan` afterwards so the row reappears in `status`
        // even when there was no cached result to update in place.
        //
        // `testOne` always tests whichever endpoint is CURRENTLY ACTIVE (there is one shared
        // connectivity-test call, not one per row - see `test_provider`), so the key it
        // stores the result under here must match: this row's own id when it IS the active
        // one, falling back to the bare kind only in the edge case of viewing a vendor's
        // registry while a DIFFERENT vendor is actually active (verifying there is a no-op
        // against the wrong row either way, pending the "test a specific non-active
        // endpoint" capability this does not yet have).
        onVerify={async () => {
          await testOne(selectedRow ? customVariantId(selectedRow.id) : 'custom')
          rescan()
        }}
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
      {/* Auto-fit, not a fixed column count. This used to be pinned to the exact tile count
          (3 across gated, 2 x 2 in Settings) so the grid was always a complete rectangle -
          which worked while there was one free-key tile standing in for "custom", but Groq
          and Ollama each getting their OWN tile (see below) makes 5 tiles in Settings, and a
          fixed count would either leave a dangling single cell on its own row or force an
          arbitrary column count that stops fitting the moment a third preset exists. Auto-fit
          wraps cleanly at any tile count instead. */}
      <div style={{
        display: 'grid', gap: 10,
        gridTemplateColumns: 'repeat(auto-fit, minmax(190px, 1fr))',
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
        {/* The free options, NAMED AND SEPARATE. "Bring your own API key / ADVANCED"
            described the plumbing rather than the offer: someone with no subscription - the
            exact person these tiles are for - could not tell it was the free path, and
            "advanced" warns them off it.
            Every OFFERED preset gets its own tile - they used to share one collapsed
            "custom" tile that could only ever name ONE of them, which made a second
            preset invisible the moment the first was configured. Claude/Codex/Cursor each
            get their own rather than sharing "a coding agent"; cloud presets get the same
            treatment. `gridPresets` also carries any vendor the user already configured
            before it stopped being offered (Groq, today) - see its own comment - so an
            existing endpoint never just disappears from the screen.
            Hidden under the gate, which already asks this exact question via its own
            chooser (see `gateAnswer === 'free'` above). */}
        {!gate && gridPresets.map((preset) => (
          <CloudPresetTile
            key={preset.vendor}
            preset={preset}
            value={value}
            selectedCustomId={selectedCustomId ?? null}
            status={status}
            custom={custom}
            onOpen={() => { setOpenId('custom'); setOpenVendor(preset.vendor) }}
          />
        ))}
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

      {/* Same idea for a discontinued cloud vendor (Groq): its tile is gone from the grid
          above on purpose, so this is the only place left that says what's actually
          selected. Amber, not neutral - unlike a legacy Copilot pick this one is genuinely
          not working (Rust refuses every call to it), not just "not the top recommendation". */}
      {usingHiddenCloud && activeRow && (
        <p style={{ fontSize: 11, lineHeight: 1.45, color: 'var(--color-state-pending)' }}>
          You&apos;re currently using {activeRow.name}, which is discontinued - Meridian isn&apos;t
          sending it any requests. Add a free Ollama key above to resume summaries.
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

/** ONE vendor's registry rows — what THAT preset's tile opens once a key is already on file
 *  for it. Scoped to a single preset (`providers` is pre-filtered by the caller to that
 *  vendor's rows), never the whole cross-vendor registry - Groq's key and Ollama's key are
 *  two separate things a user came here to manage one of, not one combined list.
 *
 *  ONE HEADING, ONE ROW PER KEY ON THIS VENDOR, AND ONE WAY TO ADD ANOTHER. What used to be
 *  here (long before Ollama existed at all): a title, a two-clause paragraph about billing, a
 *  grid of 232px tiles each carrying a status pill, a model, a rung label, a quota notice and
 *  two shouting buttons, and a dashed "+ Add a custom endpoint" tile. All of it described a
 *  configuration surface that no longer exists - there is exactly one way to get a key on a
 *  vendor (<CloudKeySetup>, opened directly from that vendor's own tile) and, usually,
 *  exactly one key per vendor.
 *
 *  The billing paragraph went with the add path it warned about: "any other OpenAI-compatible
 *  endpoint bills you per call" is advice for a decision this screen no longer offers, and it
 *  was the loudest text on a screen whose actual subject is free. */
function CustomDetail({
  preset, providers, value, selectedCustomId, live, statusDetail, custom, onBack, onPick,
  onAddAnother, onVerify,
}: {
  preset: CloudKeyPreset
  /** This vendor's OWN rows only - pre-filtered by the caller. */
  providers: CustomProviderView[]
  value: LlmProviderId
  selectedCustomId: string | null
  /** Whether the shared phase ladder says this provider is answering - see `ChooserTile`. */
  live: boolean
  /** Why not, verbatim from the recorded test. Undefined when nothing is wrong. */
  statusDetail?: string
  custom: ReturnType<typeof useCustomProviders>
  onBack: () => void
  onPick: (id: string) => void
  /** Open this SAME preset's wizard again, for a second key on this one vendor - a personal
   *  key and a work key, say. Never a vendor chooser: the vendor is already fixed by which
   *  tile got here. `undefined` when this vendor isn't offered for a NEW key any more (a
   *  retired preset like Groq) - the existing row(s) stay fully manageable, but nothing
   *  invites a second one. */
  onAddAnother?: () => void
  /** Run the real connectivity test and refresh what every surface reads. */
  onVerify: () => Promise<void>
}) {
  return (
    <div className="flex flex-col" style={{ gap: 14 }}>
      <BackLink onClick={onBack}>All providers</BackLink>

      <div className="flex flex-col" style={{ gap: 3 }}>
        <span style={{ fontSize: 18, fontWeight: 600, color: 'var(--t-title)', letterSpacing: '-.01em' }}>
          Your {preset.name} key
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
        {providers.map((c) => (
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

      {onAddAnother && (
        <button onClick={onAddAnother} className="self-start flex items-center"
          style={{
            gap: 6, fontSize: 12, fontWeight: 600, color: 'var(--t-accent)',
            background: 'none', border: 'none', padding: 0, cursor: 'pointer',
          }}>
          <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor"
            strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M8 3v10M3 8h10" />
          </svg>
          Add another {preset.name} key
        </button>
      )}
    </div>
  )
}

/** The chooser between the curated no-subscription presets (Groq, Ollama) - shown ONLY on the
 *  gate's "free" answer, where there is no tile yet to have already named the vendor. Settings
 *  never reaches this: Groq and Ollama each have their own tile there (`CloudPresetTile`), so
 *  clicking one already answers "which vendor" and skips straight to its wizard or registry.
 *
 *  A dedicated small chooser rather than reusing `<ChooserTile>`: that component is built
 *  around a SELECTED `LlmProviderId`'s live status (IN USE / NOT CONNECTED, a `Phase`) and
 *  neither concept applies to picking which vendor to configure NEXT - there is nothing
 *  selected or live yet, only a choice about what to set up. */
function CloudPresetChooser({ onBack, onPick }: {
  onBack: () => void
  onPick: (preset: CloudKeyPreset) => void
}) {
  return (
    <div className="flex flex-col" style={{ gap: 12 }}>
      <BackLink onClick={onBack}>Back</BackLink>
      <div>
        <h2 style={{ fontSize: 17, fontWeight: 600, color: 'var(--t-title)', lineHeight: 1.3 }}>
          Choose a free provider
        </h2>
        <p style={{ fontSize: 12, lineHeight: 1.5, color: 'var(--t-muted)', marginTop: 5 }}>
          Both are free, no card required. Meridian sets either one up from a single pasted key.
        </p>
      </div>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(200px, 1fr))', gap: 11 }}>
        {CLOUD_PRESETS.map((preset) => (
          <button
            key={preset.vendor}
            onClick={() => onPick(preset)}
            className="flex flex-col text-left mer-pop"
            style={{
              gap: 10, padding: '15px 16px', borderRadius: 13, cursor: 'pointer',
              background: 'var(--t-card)', border: '1px solid var(--t-ctrl-border)',
              boxShadow: '0 1px 2px rgba(0,0,0,.04)',
            }}>
            <span className="flex items-center justify-center shrink-0" style={{
              width: 32, height: 32, borderRadius: 9,
              background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
            }}>
              <CustomVendorLogo vendor={preset.vendor} size={17} />
            </span>
            <div className="flex flex-col" style={{ gap: 5 }}>
              <div className="flex items-center flex-wrap" style={{ gap: 5 }}>
                <Badge filled>{preset.freeBadge}</Badge>
                <Badge>{preset.privacyBadge}</Badge>
              </div>
              <span style={{ fontSize: 14.5, fontWeight: 600, color: 'var(--t-title)' }}>
                {preset.name}
              </span>
              <span style={{ fontSize: 11.5, lineHeight: 1.45, color: 'var(--t-muted)' }}>
                {preset.blurb}
              </span>
            </div>
          </button>
        ))}
      </div>
    </div>
  )
}
