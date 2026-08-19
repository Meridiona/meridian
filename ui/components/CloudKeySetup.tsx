//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The no-subscription path: get a free cloud API key, in three steps, and be done.
//
// One component, driven by a `CloudKeyPreset` (`@/lib/llm-providers`), for every curated
// no-subscription vendor - today Groq and Ollama. It used to be Groq-only (`GroqSetup`),
// hardcoded to that one preset's copy and URLs; generalizing it is what let a second preset
// exist without a second ~300-line copy of the same wizard logic to keep in sync.
//
// This replaces a generic add-endpoint FORM (vendor dropdown, base URL, model, rate limits,
// two quota fields) with a walkthrough, because the two surfaces are answering completely
// different questions. The form asked "describe your endpoint", which assumes the user has
// one in mind. This asks "you said you have no subscription - here is how to get set up",
// which is the actual situation of everyone who reaches it. Every field the form exposed is
// either fixed by the vendor (URL) or decided for the user (model), so none of them is a
// decision worth putting on screen.
//
// THE MODEL IS CHOSEN FROM THE ENDPOINT'S OWN LIST, not hardcoded. A vendor's catalogue
// turns over, and a retired model id baked in here would not fail at setup - it would fail at
// the first real hour, hours later, with nothing on screen connecting the two. So the key is
// used to ask the endpoint what it actually serves, and `pickCloudModel` ranks that list
// against `preset.modelPreference`.
//
// THE PRIVACY CLAIMS CARRY THEIR SOURCE. This screen asks someone to paste an API key partly
// on the strength of three sentences about data handling; a sentence with no link is exactly
// what a careful reader should refuse. Each claim sits next to the vendor's own policy page.
//
// # Who calls this
// [`LlmProviderPicker`], when the gate is answered "free" or the "bring your own key" tile is
// opened with no endpoint configured yet - in both cases after a preset (Groq or Ollama) has
// been picked from `CLOUD_PRESETS`.
//
// # Related
// - `@/lib/llm-providers` — `CloudKeyPreset`, `GROQ`, `OLLAMA`, `pickCloudModel`
// - `@/components/CustomProviders` — the registry hook this writes through

import { BackLink } from '@/components/ui/BackLink'
import { useState } from 'react'
import { invoke, openExternal } from '@/lib/bridge'
import { pickCloudModel, type CloudKeyPreset, type ProbeOutcome } from '@/lib/llm-providers'
import { CustomVendorLogo } from '@/components/LlmProviderLogos'

/** What `add` does under the hood - the registry write, unchanged from the form path. */
type AddFn = (fields: {
  vendor: string; name: string; base_url: string; model: string
  api_key: string; rpm: number; rpd: number
}) => Promise<ProbeOutcome>

type Phase = 'idle' | 'listing' | 'adding' | 'selecting' | 'done'

export default function CloudKeySetup({ preset, onBack, onAdd, onPick }: {
  preset: CloudKeyPreset
  onBack: () => void
  onAdd: AddFn
  /** Make the freshly added endpoint the live provider. Rejects if the tray refuses. */
  onPick: (customId: string) => void | Promise<void>
}) {
  const [apiKey, setApiKey] = useState('')
  const [phase, setPhase] = useState<Phase>('idle')
  const [error, setError] = useState<string | null>(null)
  const [model, setModel] = useState<string | null>(null)
  // Set once the endpoint EXISTS daemon-side, which is what makes a retry different from a
  // first attempt: the add can never be repeated (the tray rejects a duplicate name,
  // permanently), so from here on the recovery is to re-measure the endpoint we already
  // saved. Without this the screen was a dead end - see `connect`.
  const [addedId, setAddedId] = useState<string | null>(null)

  const busy = phase === 'listing' || phase === 'adding' || phase === 'selecting'
  // Once the endpoint is saved the key lives daemon-side and is not needed again, so the
  // button must not be gated on a field the flow itself no longer requires.
  const canConnect = !busy && (!!apiKey.trim() || !!addedId)

  // THE FAILURE THIS IS SHAPED AROUND is a free-tier 429 during the eligibility probe -
  // the expected case on a free tier, not a rare one - landing on the COMPULSORY
  // AI-connect step of first-run.
  //
  // It used to be unrecoverable. The key was cleared as soon as the endpoint was saved but
  // before eligibility was checked, so the throw left an empty field; `connect` opens by
  // returning early on an empty key, so pressing the button again did nothing at all -
  // no request, no error, no change. And even with the key retyped, a retry re-ran the add,
  // which the tray refuses forever once an endpoint with this name exists. There was no way
  // out from inside the flow either: the picker renders this screen unconditionally for the
  // "free" gate answer and Back returns to that same gate.
  //
  // So: the key is held until the whole flow succeeds, and a retry after the save re-measures
  // the existing endpoint instead of trying to create a second one.
  async function connect() {
    const key = apiKey.trim()
    if (!key && !addedId) return
    setError(null)
    try {
      let outcome: ProbeOutcome
      if (addedId) {
        // RETRY, endpoint already saved. Re-measure rather than re-add: `refresh: true` runs
        // the same schema probe the add ran, which is the step a 429 interrupted, and it is
        // the only one that needs repeating. The key is not passed because the daemon holds
        // it - which is also why the field being empty here is correct rather than a problem.
        setPhase('adding')
        outcome = await invoke<ProbeOutcome>('probe_custom_llm_provider', {
          id: addedId,
          refresh: true,
        })
      } else {
        // 1. Ask the endpoint what it serves, on this key. Doubles as the first real check
        //    that the key works at all - a typo fails here, before anything is written to
        //    settings.
        setPhase('listing')
        const ids = await invoke<string[]>('list_custom_llm_provider_models', {
          baseUrl: preset.baseUrl,
          apiKey: key,
        })
        const chosen = pickCloudModel(preset, ids)
        if (!chosen) {
          // Empty, or nothing but speech/embedding models. There is no default to fall back
          // on, so say so rather than writing an endpoint that cannot answer a prompt.
          throw new Error('This key has no chat models available on it. Check the key and try again.')
        }
        setModel(chosen)

        // 2. Save + MEASURE it. `add` runs the real schema probe, which is what decides
        //    whether Meridian can actually use it - see `production_eligible`.
        setPhase('adding')
        outcome = await onAdd({
          vendor: preset.vendor, name: preset.name, base_url: preset.baseUrl,
          model: chosen, api_key: key,
          // The preset's published free-tier limits for the model we just pinned - never a
          // guess, and never silently invented either: `0/0` (Ollama) means "not known", not
          // "unmetered". See `freeRpm`/`freeRpd` on `CloudKeyPreset`.
          rpm: preset.freeRpm, rpd: preset.freeRpd,
        })
        // Recorded BEFORE the eligibility check below, because it is true regardless of how
        // that check goes: the endpoint exists now, so every later attempt must re-measure
        // this id rather than try to create it again.
        setAddedId(outcome.provider.id)
      }

      if (!outcome.provider.production_eligible) {
        throw new Error(
          `${outcome.provider.model} answered, but it could not return the structured replies ` +
          `Meridian needs. This is often a busy free tier - press Connect again to try again.`,
        )
      }

      // 3. Make it the live provider. Separate from the save on purpose: the tray REJECTS
      //    selecting an ineligible endpoint, so this can fail even though the add worked.
      setPhase('selecting')
      await onPick(outcome.provider.id)
      setApiKey('')  // stored daemon-side now, and it never comes back
      setPhase('done')
    } catch (e) {
      setError(String(e).replace(/^Error:\s*/, ''))
      setPhase('idle')
    }
  }

  if (phase === 'done') {
    return (
      <div className="flex flex-col mer-pop" style={{ gap: 10, alignItems: 'flex-start' }}>
        <HueBadge hue="var(--color-state-approved)">CONNECTED</HueBadge>
        <span style={{ fontSize: 17, fontWeight: 600, color: 'var(--t-title)' }}>
          You&apos;re set up
        </span>
        <p style={{ fontSize: 12, lineHeight: 1.5, color: 'var(--t-muted)', maxWidth: 480 }}>
          Meridian is using {preset.name}{model ? ` (${model})` : ''} to write your summaries.
          You can change this any time in Settings.
        </p>
      </div>
    )
  }

  return (
    <div className="flex flex-col" style={{ gap: 14 }}>
      <BackLink onClick={onBack}>Back</BackLink>

      {/* The two badges lead, together. FREE is what makes this the right screen for
          someone who just said they have no subscription; the privacy badge is the other
          half of the same decision, and it used to sit below the fold in a block that
          reads as boilerplate. Both are one-word answers to the two questions actually
          being asked, so both are stated before anything else. */}
      <div className="flex flex-col" style={{ gap: 7 }}>
        <div className="flex items-center" style={{ gap: 6 }}>
          <HueBadge hue="var(--color-state-approved)">{preset.freeBadge}</HueBadge>
          <HueBadge hue="var(--t-accent)">{preset.privacyBadge}</HueBadge>
        </div>
        <span className="flex items-center" style={{ gap: 9, fontSize: 19, fontWeight: 600, color: 'var(--t-title)' }}>
          <span className="flex items-center justify-center shrink-0" style={{
            width: 32, height: 32, borderRadius: 9,
            background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
          }}>
            <CustomVendorLogo vendor={preset.vendor} size={17} />
          </span>
          {preset.headline}
        </span>
        <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-muted)', maxWidth: 500 }}>
          {preset.blurb}
        </p>
      </div>

      {/* Data handling, with the vendor's own pages attached. */}
      <div className="flex flex-col" style={{
        gap: 7, padding: '13px 14px', borderRadius: 11,
        background: 'color-mix(in srgb, var(--color-state-approved) 6%, transparent)',
        border: '1px solid color-mix(in srgb, var(--color-state-approved) 24%, transparent)',
      }}>
        <span className="font-mono" style={{
          fontSize: 9, letterSpacing: '.1em', color: 'var(--color-state-approved)',
        }}>YOUR DATA</span>
        {preset.trust.map((line) => (
          <div key={line} className="flex items-start" style={{ gap: 7 }}>
            <span aria-hidden style={{ color: 'var(--color-state-approved)', fontSize: 11, marginTop: 1 }}>✓</span>
            <span style={{ fontSize: 11.5, lineHeight: 1.45, color: 'var(--t-muted)' }}>{line}</span>
          </div>
        ))}
        <div className="flex items-center" style={{ gap: 12, marginTop: 3 }}>
          <LinkOut href={preset.privacyUrl}>{preset.name} privacy policy</LinkOut>
          <LinkOut href={preset.termsUrl}>Terms</LinkOut>
        </div>
      </div>

      {/* ONE link, to the keys page - not a sign-up step and then a key step. Every preset's
          key page already handles both: signed out it asks you to sign up (Google or email,
          no card), then lands you on the very page the step is asking for. Splitting that
          into two numbered steps described the same click twice and made a two-minute setup
          look like a three-part chore. */}
      <Step n={1} title={`Create a free ${preset.name} API key`}
        body={preset.keyStepBody}>
        <ActionLink href={preset.keyUrl}>Open {preset.name} API keys</ActionLink>
      </Step>

      <Step n={2} title="Paste it here"
        body="Meridian picks the model, checks the key works, and sets everything else up.">
        <div className="flex flex-col" style={{ gap: 8 }}>
          <input
            data-tour="cloud-key"
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={preset.keyPlaceholder}
            spellCheck={false}
            autoComplete="off"
            disabled={busy}
            style={{
              fontSize: 12, padding: '8px 10px', borderRadius: 8, width: '100%',
              border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)',
              color: 'var(--t-title)', fontFamily: 'var(--font-mono, ui-monospace)',
              opacity: busy ? 0.6 : 1,
            }}
          />
          <button
            data-tour="cloud-connect"
            onClick={connect}
            disabled={!canConnect}
            className="self-start"
            style={{
              fontSize: 12, fontWeight: 700, color: '#fff', padding: '8px 14px',
              borderRadius: 9, border: 'none',
              background: 'var(--btn-primary-bg)',
              opacity: canConnect ? 1 : 0.5,
              cursor: canConnect ? 'pointer' : 'default',
            }}>
            {phase === 'listing' ? 'Checking your key…'
              : phase === 'adding' ? 'Setting up…'
                : phase === 'selecting' ? 'Almost there…'
                  : `Connect ${preset.name}`}
          </button>
          {/* Named, because "Setting up…" can sit for a while - it is running real
              requests against the endpoint to measure what it supports. */}
          {phase === 'adding' && (
            <span style={{ fontSize: 10.5, lineHeight: 1.4, color: 'var(--t-faint)' }}>
              Checking which replies {model} can give - this takes a few seconds.
            </span>
          )}
          {error && (
            <p style={{
              fontSize: 11.5, lineHeight: 1.45, color: 'var(--color-state-pending)',
              background: 'color-mix(in srgb, var(--color-state-pending) 10%, transparent)',
              borderRadius: 8, padding: '8px 10px',
            }}>{error}</p>
          )}
        </div>
      </Step>
    </div>
  )
}

/** One numbered step. The number is the whole navigation aid here - there is no progress
 *  bar, because all three are on screen at once and the user can see where they are. */
function Step({ n, title, body, children }: {
  n: number; title: string; body: string; children: React.ReactNode
}) {
  return (
    <div className="flex items-start" style={{ gap: 11 }}>
      <span className="flex items-center justify-center shrink-0 font-mono" style={{
        width: 22, height: 22, borderRadius: 999, fontSize: 10.5, fontWeight: 700,
        background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
        color: 'var(--t-title)', marginTop: 1,
      }}>{n}</span>
      <div className="flex flex-col min-w-0 flex-1" style={{ gap: 5 }}>
        <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--t-title)' }}>{title}</span>
        <span style={{ fontSize: 11.5, lineHeight: 1.45, color: 'var(--t-muted)' }}>{body}</span>
        {children}
      </div>
    </div>
  )
}

function ActionLink({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <button onClick={() => openExternal(href)} className="self-start flex items-center"
      style={{
        gap: 5, fontSize: 11.5, fontWeight: 600, color: 'var(--t-title)',
        padding: '6px 11px', borderRadius: 8, cursor: 'pointer',
        background: 'var(--t-ctrl)', border: '1px solid var(--t-ctrl-border)',
      }}>
      {children}
      <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
        <path d="M6 3h7v7M13 3 4 12" />
      </svg>
    </button>
  )
}

function LinkOut({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <button onClick={() => openExternal(href)}
      style={{
        fontSize: 10.5, color: 'var(--t-muted)', background: 'none', border: 'none',
        padding: 0, cursor: 'pointer', textDecoration: 'underline',
      }}>{children}</button>
  )
}

/** A hue-tinted pill: the colour is the message (green = free, accent = privacy).
 *  Named HueBadge, not Badge, because `LlmProviderGate` exports a DIFFERENT
 *  `Badge` - a neutral filled/outline chip with no colour input. Two components
 *  with one name in one flow is a real import hazard, and they should not be
 *  merged: a single component taking both `filled` and `hue` would have two
 *  mutually exclusive modes and no way to express that in its type. */
function HueBadge({ children, hue }: { children: React.ReactNode; hue: string }) {
  return (
    <span className="font-mono self-start" style={{
      fontSize: 9, letterSpacing: '.1em', color: hue,
      border: `1px solid ${hue}`,
      background: `color-mix(in srgb, ${hue} 10%, transparent)`,
      borderRadius: 4, padding: '2px 7px', whiteSpace: 'nowrap',
    }}>{children}</span>
  )
}
