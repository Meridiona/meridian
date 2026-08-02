//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The no-subscription path: get a free Groq key, in three steps, and be done.
//
// This replaces a generic add-endpoint FORM (vendor dropdown, base URL, model, rate limits,
// two quota fields) with a walkthrough, because the two surfaces are answering completely
// different questions. The form asked "describe your endpoint", which assumes the user has
// one in mind. This asks "you said you have no subscription - here is how to get set up",
// which is the actual situation of everyone who reaches it. Every field the form exposed is
// either fixed by the vendor (URL) or decided for the user (model), so none of them is a
// decision worth putting on screen.
//
// THE MODEL IS CHOSEN FROM THE ENDPOINT'S OWN LIST, not hardcoded. Groq's catalogue turns
// over, and a retired model id baked in here would not fail at setup - it would fail at the
// first real hour, hours later, with nothing on screen connecting the two. So the key is
// used to ask Groq what it actually serves, and `pickGroqModel` ranks that list. See
// GROQ_MODEL_PREFERENCE.
//
// THE PRIVACY CLAIMS CARRY THEIR SOURCE. This screen asks someone to paste an API key partly
// on the strength of three sentences about data handling; a sentence with no link is exactly
// what a careful reader should refuse. Each claim sits next to Groq's own policy page.
//
// # Who calls this
// [`LlmProviderPicker`], when the gate is answered "free".
//
// # Related
// - `@/lib/llm-providers` — GROQ (copy + urls), pickGroqModel
// - `@/components/CustomProviders` — the registry hook this writes through

import { useState } from 'react'
import { invoke, openExternal } from '@/lib/bridge'
import { GROQ, pickGroqModel, type ProbeOutcome } from '@/lib/llm-providers'
import { GroqLogo } from '@/components/LlmProviderLogos'

/** What `add` does under the hood - the registry write, unchanged from the form path. */
type AddFn = (fields: {
  vendor: string; name: string; base_url: string; model: string
  api_key: string; rpm: number; rpd: number
}) => Promise<ProbeOutcome>

type Phase = 'idle' | 'listing' | 'adding' | 'selecting' | 'done'

export default function GroqSetup({ onBack, onAdd, onPick }: {
  onBack: () => void
  onAdd: AddFn
  /** Make the freshly added endpoint the live provider. Rejects if the tray refuses. */
  onPick: (customId: string) => void | Promise<void>
}) {
  const [apiKey, setApiKey] = useState('')
  const [phase, setPhase] = useState<Phase>('idle')
  const [error, setError] = useState<string | null>(null)
  const [model, setModel] = useState<string | null>(null)

  const busy = phase === 'listing' || phase === 'adding' || phase === 'selecting'

  async function connect() {
    const key = apiKey.trim()
    if (!key) return
    setError(null)
    try {
      // 1. Ask Groq what it serves, on this key. Doubles as the first real check that the
      //    key works at all - a typo fails here, before anything is written to settings.
      setPhase('listing')
      const ids = await invoke<string[]>('list_custom_llm_provider_models', {
        baseUrl: GROQ.baseUrl,
        apiKey: key,
      })
      const chosen = pickGroqModel(ids)
      if (!chosen) {
        // Empty, or nothing but speech/embedding models. There is no default to fall back
        // on, so say so rather than writing an endpoint that cannot answer a prompt.
        throw new Error('This key has no chat models available on it. Check the key is from console.groq.com and try again.')
      }
      setModel(chosen)

      // 2. Save + MEASURE it. `add` runs the real schema probe, which is what decides
      //    whether Meridian can actually use it - see `production_eligible`.
      setPhase('adding')
      const outcome = await onAdd({
        vendor: GROQ.vendor, name: GROQ.name, base_url: GROQ.baseUrl,
        model: chosen, api_key: key,
        // Unknown rather than guessed. Groq's free limits differ per model and change; a
        // made-up number here would drive a capacity verdict the user would read as fact.
        rpm: 0, rpd: 0,
      })
      setApiKey('')  // stored daemon-side now, and it never comes back

      if (!outcome.provider.production_eligible) {
        throw new Error(
          `${chosen} answered, but it could not return the structured replies Meridian needs. ` +
          `The key is saved - open All providers to try measuring it again.`,
        )
      }

      // 3. Make it the live provider. Separate from the save on purpose: the tray REJECTS
      //    selecting an ineligible endpoint, so this can fail even though the add worked.
      setPhase('selecting')
      await onPick(outcome.provider.id)
      setPhase('done')
    } catch (e) {
      setError(String(e).replace(/^Error:\s*/, ''))
      setPhase('idle')
    }
  }

  if (phase === 'done') {
    return (
      <div className="flex flex-col mer-pop" style={{ gap: 10, alignItems: 'flex-start' }}>
        <Badge hue="var(--color-state-approved)">CONNECTED</Badge>
        <span style={{ fontSize: 17, fontWeight: 600, color: 'var(--t-title)' }}>
          You&apos;re set up
        </span>
        <p style={{ fontSize: 12, lineHeight: 1.5, color: 'var(--t-muted)', maxWidth: 480 }}>
          Meridian is using Groq{model ? ` (${model})` : ''} to write your summaries. You can
          change this any time in Settings.
        </p>
      </div>
    )
  }

  return (
    <div className="flex flex-col" style={{ gap: 14 }}>
      <button onClick={onBack} className="self-start flex items-center"
        style={{ gap: 6, fontSize: 12, color: 'var(--t-muted)', background: 'none', border: 'none', cursor: 'pointer', padding: 0 }}>
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="M10 4 6 8l4 4" /></svg>
        Back
      </button>

      {/* The free badge leads. It is the single fact that makes this the right screen for
          someone who just said they have no subscription. */}
      <div className="flex flex-col" style={{ gap: 7 }}>
        <Badge hue="var(--color-state-approved)">{GROQ.freeBadge}</Badge>
        <span className="flex items-center" style={{ gap: 9, fontSize: 19, fontWeight: 600, color: 'var(--t-title)' }}>
          <span className="flex items-center justify-center shrink-0" style={{
            width: 32, height: 32, borderRadius: 9,
            background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
          }}>
            <GroqLogo size={17} />
          </span>
          {GROQ.headline}
        </span>
        <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-muted)', maxWidth: 500 }}>
          {GROQ.blurb}
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
        {GROQ.trust.map((line) => (
          <div key={line} className="flex items-start" style={{ gap: 7 }}>
            <span aria-hidden style={{ color: 'var(--color-state-approved)', fontSize: 11, marginTop: 1 }}>✓</span>
            <span style={{ fontSize: 11.5, lineHeight: 1.45, color: 'var(--t-muted)' }}>{line}</span>
          </div>
        ))}
        <div className="flex items-center" style={{ gap: 12, marginTop: 3 }}>
          <LinkOut href={GROQ.privacyUrl}>Groq privacy policy</LinkOut>
          <LinkOut href={GROQ.termsUrl}>Terms</LinkOut>
        </div>
      </div>

      <Step n={1} title="Create a free Groq account"
        body="Sign up with Google or an email address. No card is asked for.">
        <ActionLink href={GROQ.signUpUrl}>Open console.groq.com</ActionLink>
      </Step>

      <Step n={2} title="Create an API key"
        body="On the API Keys page, press Create API key. Copy it - Groq shows it once.">
        <ActionLink href={GROQ.keyUrl}>Open API keys</ActionLink>
      </Step>

      <Step n={3} title="Paste it here"
        body="Meridian picks the model, checks the key works, and sets everything else up.">
        <div className="flex flex-col" style={{ gap: 8 }}>
          <input
            data-tour="groq-key"
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={GROQ.keyPlaceholder}
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
            data-tour="groq-connect"
            onClick={connect}
            disabled={busy || !apiKey.trim()}
            className="self-start"
            style={{
              fontSize: 12, fontWeight: 700, color: '#fff', padding: '8px 14px',
              borderRadius: 9, border: 'none',
              background: 'var(--color-state-proposal)',
              opacity: busy || !apiKey.trim() ? 0.5 : 1,
              cursor: busy || !apiKey.trim() ? 'default' : 'pointer',
            }}>
            {phase === 'listing' ? 'Checking your key…'
              : phase === 'adding' ? 'Setting up…'
                : phase === 'selecting' ? 'Almost there…'
                  : 'Connect Groq'}
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

function Badge({ children, hue }: { children: React.ReactNode; hue: string }) {
  return (
    <span className="font-mono self-start" style={{
      fontSize: 9, letterSpacing: '.1em', color: hue,
      border: `1px solid ${hue}`,
      background: `color-mix(in srgb, ${hue} 10%, transparent)`,
      borderRadius: 4, padding: '2px 7px', whiteSpace: 'nowrap',
    }}>{children}</span>
  )
}
