//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The per-provider DETAIL view — what opens when a provider tile is clicked in the chooser.
// One provider, one screen: install its CLI if it's missing, confirm the sign-in if it's
// there, optionally set its model, and make it the default. Kept deliberately linear so a
// non-technical user is only ever looking at one next action.
//
// The three coding-agent CLIs run on a subscription the user already has, so there is no API
// key to paste here — the only setup is "install the CLI" and "be signed in to it", and this
// view walks both. The custom (bring-your-own-key) case has its own flow and is NOT rendered
// here (see <LlmProviderPicker>'s custom detail).

import { useEffect, useRef, useState } from 'react'
import { openExternal } from '@/lib/bridge'
import { ProviderLogo } from '@/components/LlmProviderLogos'
import { LLM_RECOMMENDED_BADGE, USAGE_FOOTPRINT_NOTE, type LlmProviderMeta } from '@/lib/llm-providers'
import type { InstallOutcome, ProviderStatus, ProviderTestResult } from '@/components/LlmProviderPicker'

/** The connection state we render, derived from install + last-test + in-flight flags. */
type Phase =
  | { kind: 'installing' }
  | { kind: 'signing' }
  | { kind: 'not_installed' }
  | { kind: 'testing' }
  | { kind: 'ok' }
  | { kind: 'failed'; message: string }
  | { kind: 'rate_limited'; message: string }
  | { kind: 'ready_untested' }
  | { kind: 'unknown' }

function phaseFor(
  installing: boolean,
  signing: boolean,
  testing: boolean,
  probed: ProviderStatus | undefined,
  lastTest: ProviderTestResult | null,
): Phase {
  if (installing) return { kind: 'installing' }
  if (signing) return { kind: 'signing' }
  if (testing) return { kind: 'testing' }
  // Only say "not installed" when we actually probed and came up empty — a failed probe is
  // "unknown", not "missing" (picking/installing is still allowed).
  if (probed && !probed.installed) return { kind: 'not_installed' }
  if (lastTest) {
    if (lastTest.outcome.status === 'ok') return { kind: 'ok' }
    if (lastTest.outcome.status === 'rate_limited')
      return { kind: 'rate_limited', message: lastTest.outcome.message }
    return { kind: 'failed', message: lastTest.outcome.message }
  }
  if (probed?.installed) return { kind: 'ready_untested' }
  return { kind: 'unknown' }
}

function Dot({ color }: { color: string }) {
  return <span style={{ width: 7, height: 7, borderRadius: 99, background: color, flexShrink: 0, marginTop: 6 }} />
}

function Spinner() {
  return (
    <span className="inline-block" style={{
      width: 13, height: 13, borderRadius: 99, marginTop: 3, flexShrink: 0,
      border: '2px solid var(--t-ctrl-border)', borderTopColor: 'var(--color-state-proposal)',
      animation: 'spin 0.7s linear infinite',
    }} />
  )
}

export interface LlmProviderDetailProps {
  p: LlmProviderMeta
  probed: ProviderStatus | undefined
  testing: boolean
  installing: boolean
  signing: boolean
  /** This provider is the one currently saved/in use. */
  selected: boolean
  onBack: () => void
  /** Run the vendor installer; resolves with what happened so we can show the message. */
  onInstall: () => Promise<InstallOutcome>
  /** Run the interactive Cursor sign-in (browser OAuth on their subscription). Cursor-only. */
  onSignIn: () => Promise<InstallOutcome>
  onTest: () => void
  /** Commit this provider as the default. Rejects if the write failed (Settings save). */
  onUse: () => Promise<void>
}

export default function LlmProviderDetail({
  p, probed, testing, installing, signing, selected, onBack, onInstall, onSignIn, onTest, onUse,
}: LlmProviderDetailProps) {
  const lastTest = probed?.last_test ?? null
  const phase = phaseFor(installing, signing, testing, probed, lastTest)
  const [installMsg, setInstallMsg] = useState<string | null>(null)
  const [signInMsg, setSignInMsg] = useState<string | null>(null)
  const [applying, setApplying] = useState(false)
  const [applyErr, setApplyErr] = useState<string | null>(null)

  // Auto-test once when we open on an installed CLI with no recent result — "if already
  // installed, test automatically". Guarded so a status refresh can't re-fire it, and skipped
  // when a test ran in the last minute so flipping back and forth doesn't burn requests.
  const autoTested = useRef<string | null>(null)
  useEffect(() => {
    if (autoTested.current === p.id) return
    if (installing || testing) return
    if (!probed?.installed) return
    const recent = lastTest && Date.now() - Date.parse(lastTest.tested_at) < 60_000
    if (recent) { autoTested.current = p.id; return }
    autoTested.current = p.id
    onTest()
  }, [p.id, probed?.installed, installing, testing, lastTest, onTest])

  async function install() {
    setInstallMsg(null)
    const outcome = await onInstall()
    // On success the hook re-detects and auto-tests, so the panel moves on by itself; only a
    // failure needs its message surfaced here.
    if (!outcome.ok) setInstallMsg(outcome.message)
    else autoTested.current = p.id // the hook already tested; don't double-fire the effect
  }

  async function signIn() {
    setSignInMsg(null)
    const outcome = await onSignIn()
    if (!outcome.ok) setSignInMsg(outcome.message)
    else autoTested.current = p.id // the hook re-tested after sign-in
  }

  async function use() {
    setApplying(true); setApplyErr(null)
    try { await onUse() } catch (e) { setApplyErr(String(e)) } finally { setApplying(false) }
  }

  return (
    <div className="flex flex-col" style={{ gap: 14 }}>
      {/* Back to the chooser */}
      <button onClick={onBack} className="self-start flex items-center"
        style={{ gap: 6, fontSize: 12, color: 'var(--t-muted)', background: 'none', border: 'none', cursor: 'pointer', padding: 0 }}>
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="M10 4 6 8l4 4" /></svg>
        All providers
      </button>

      {/* Header */}
      <div className="flex items-center" style={{ gap: 13 }}>
        <span className="flex items-center justify-center shrink-0" style={{
          width: 46, height: 46, borderRadius: 12,
          background: selected ? 'color-mix(in srgb, var(--color-state-proposal) 15%, transparent)' : 'var(--t-box)',
          border: '1px solid var(--t-ctrl-border)',
          color: selected ? 'var(--color-state-proposal)' : 'var(--t-title)',
        }}>
          <ProviderLogo id={p.id} size={24} />
        </span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div className="flex items-center flex-wrap" style={{ gap: 8 }}>
            <span style={{ fontSize: 17, fontWeight: 600, color: 'var(--t-title)' }}>{p.name}</span>
            <span className="font-mono" style={{
              fontSize: 9.5, letterSpacing: '.09em', color: 'var(--color-state-proposal)',
              border: '1px solid var(--color-state-proposal)',
              background: 'color-mix(in srgb, var(--color-state-proposal) 10%, transparent)',
              borderRadius: 4, padding: '1.5px 5px',
            }}>{LLM_RECOMMENDED_BADGE}</span>
            {selected && (
              <span className="font-mono flex items-center" style={{
                gap: 4, fontSize: 9.5, letterSpacing: '.09em', color: 'var(--color-state-approved)',
                border: '1px solid var(--color-state-approved)',
                background: 'color-mix(in srgb, var(--color-state-approved) 10%, transparent)',
                borderRadius: 4, padding: '1.5px 5px',
              }}>IN USE</span>
            )}
          </div>
          <p style={{ fontSize: 12, lineHeight: 1.45, color: 'var(--t-muted)', marginTop: 3 }}>{p.blurb}</p>
        </div>
      </div>

      {/* Connection panel — one clear next action */}
      <div className="flex flex-col" style={{
        gap: 10, padding: '14px 15px', borderRadius: 12,
        background: 'var(--t-card)', border: '1px solid var(--t-ctrl-border)',
      }}>
        <ConnectionBody
          phase={phase}
          name={p.name}
          providerId={p.id}
          installHint={p.installHint}
          installMsg={installMsg}
          signInMsg={signInMsg}
          onInstall={install}
          onSignIn={signIn}
          onTest={onTest}
        />
      </div>

      {/* Privacy — honest: we disable telemetry per call, but opting out of TRAINING is a
          one-time account setting we can't flip. */}
      {p.privacyUrl && (
        <div className="flex items-start" style={{ gap: 8 }}>
          <span className="shrink-0" style={{ marginTop: 1, color: 'var(--color-state-approved)' }} aria-hidden="true">🔒</span>
          <p style={{ fontSize: 11, lineHeight: 1.5, color: 'var(--t-muted)' }}>
            Meridian turns off usage telemetry on every call. To also keep your prompts out of model
            training, switch on {p.name}&apos;s privacy setting once:{' '}
            <button onClick={() => p.privacyUrl && openExternal(p.privacyUrl)}
              style={{ color: 'var(--color-state-proposal)', background: 'none', border: 'none', padding: 0, cursor: 'pointer', font: 'inherit', textDecoration: 'underline' }}>
              {p.privacyLabel ?? 'open settings'} ↗
            </button>
          </p>
        </div>
      )}

      {/* Primary action */}
      <div className="flex flex-col" style={{ gap: 6 }}>
        {applyErr && <p style={{ fontSize: 11, lineHeight: 1.4, color: 'var(--status-error-dot)' }}>{applyErr}</p>}
        {selected ? (
          <div className="flex items-center self-start" style={{
            gap: 7, fontSize: 13, fontWeight: 500, color: 'var(--color-state-approved)',
            padding: '9px 15px', borderRadius: 9,
            background: 'color-mix(in srgb, var(--color-state-approved) 10%, transparent)',
            border: '1px solid var(--color-state-approved)',
          }}>
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M13 4 6 11 3 8" /></svg>
            {p.name} is writing your summaries
          </div>
        ) : (
          <button onClick={use} disabled={applying} className="self-start"
            style={{
              fontSize: 13, fontWeight: 600, padding: '9px 18px', borderRadius: 9, border: 'none',
              background: applying ? 'var(--t-ctrl)' : 'var(--btn-primary-bg)',
              color: applying ? 'var(--t-faint)' : '#fff', cursor: applying ? 'default' : 'pointer',
            }}>
            {applying ? 'Switching…' : `Use ${p.name}`}
          </button>
        )}
        <p style={{ fontSize: 11, lineHeight: 1.45, color: 'var(--t-muted)' }}>
          {USAGE_FOOTPRINT_NOTE}
        </p>
      </div>
    </div>
  )
}

/** The body of the connection panel — one message and (at most) one button per phase. */
function ConnectionBody({ phase, name, providerId, installHint, installMsg, signInMsg, onInstall, onSignIn, onTest }: {
  phase: Phase; name: string; providerId: string
  installHint?: string; installMsg: string | null; signInMsg: string | null
  onInstall: () => void; onSignIn: () => void; onTest: () => void
}) {
  const mono = { fontSize: 11, lineHeight: 1.5, color: 'var(--t-key-text)', background: 'var(--t-key-bg)', borderRadius: 6, padding: '6px 9px' } as const
  const isCursor = providerId === 'cursor'

  switch (phase.kind) {
    case 'installing':
      return (
        <div className="flex items-start" style={{ gap: 9 }}>
          <Spinner />
          <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
            Installing the {name} CLI… this can take a minute.
          </p>
        </div>
      )

    case 'signing':
      return (
        <div className="flex items-start" style={{ gap: 9 }}>
          <Spinner />
          <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
            Opening your browser… finish signing in to Cursor, then this updates on its own.
          </p>
        </div>
      )

    case 'not_installed':
      return (
        <div className="flex flex-col" style={{ gap: 10 }}>
          <div className="flex items-start" style={{ gap: 9 }}>
            <Dot color="var(--color-state-pending)" />
            <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
              The {name} CLI isn&apos;t installed yet. Meridian can install it for you now - one click,
              nothing to configure.
            </p>
          </div>
          <button onClick={onInstall} className="self-start"
            style={{
              fontSize: 12.5, fontWeight: 600, padding: '8px 15px', borderRadius: 9, border: 'none',
              background: 'var(--btn-primary-bg)', color: '#fff', cursor: 'pointer',
            }}>
            Install {name}
          </button>
          {installHint && (
            <p style={{ fontSize: 10.5, lineHeight: 1.5, color: 'var(--t-faint)' }}>
              Runs <code style={{ fontFamily: 'var(--font-mono, ui-monospace)' }}>{installHint}</code> in your shell.
            </p>
          )}
          {installMsg && (
            <div className="flex flex-col" style={{ gap: 6 }}>
              <p style={{ fontSize: 11.5, lineHeight: 1.5, color: 'var(--status-error-dot)' }}>
                That didn&apos;t work: {installMsg}
              </p>
              {installHint && <p className="font-mono" style={mono}>{installHint}</p>}
              <p style={{ fontSize: 10.5, lineHeight: 1.5, color: 'var(--t-faint)' }}>
                Run the command above in your terminal, then come back and reopen this.
              </p>
            </div>
          )}
        </div>
      )

    case 'testing':
      return (
        <div className="flex items-start" style={{ gap: 9 }}>
          <Spinner />
          <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
            Checking your {name} sign-in…
          </p>
        </div>
      )

    case 'ok':
      return (
        <div className="flex items-start" style={{ gap: 9 }}>
          <span className="shrink-0" style={{ marginTop: 2, color: 'var(--color-state-approved)' }} aria-hidden="true">
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M13 4 6 11 3 8" /></svg>
          </span>
          <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
            Connected and ready - you&apos;re signed in to {name}.
          </p>
        </div>
      )

    case 'rate_limited':
      return (
        <div className="flex flex-col" style={{ gap: 8 }}>
          <div className="flex items-start" style={{ gap: 9 }}>
            <Dot color="var(--color-state-pending)" />
            <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
              {name} is signed in but rate-limited right now: {phase.message}. It&apos;ll pick back up on
              its own once the limit clears.
            </p>
          </div>
          <TestAgain onTest={onTest} />
        </div>
      )

    case 'failed':
      // Cursor's "failure" is almost always just "not signed in yet" - a one-time OAuth on the
      // user's own Cursor subscription (no API key). Offer it as a one-click browser sign-in,
      // with the terminal command as a fallback.
      if (isCursor) {
        return (
          <div className="flex flex-col" style={{ gap: 10 }}>
            <div className="flex items-start" style={{ gap: 9 }}>
              <Dot color="var(--color-state-pending)" />
              <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
                {name} is installed but not signed in yet. Sign in with your Cursor account and it
                runs on your Cursor subscription - no API key.
              </p>
            </div>
            <button onClick={onSignIn} className="self-start"
              style={{
                fontSize: 12.5, fontWeight: 600, padding: '8px 15px', borderRadius: 9, border: 'none',
                background: 'var(--btn-primary-bg)', color: '#fff', cursor: 'pointer',
              }}>
              Sign in to Cursor
            </button>
            <p style={{ fontSize: 10.5, lineHeight: 1.5, color: 'var(--t-faint)' }}>
              Opens your browser to finish the sign-in. Prefer the terminal? Run{' '}
              <code style={{ fontFamily: 'var(--font-mono, ui-monospace)' }}>cursor-agent login</code>, then Test again.
            </p>
            {signInMsg && (
              <p style={{ fontSize: 11.5, lineHeight: 1.5, color: 'var(--status-error-dot)' }}>
                Sign-in didn&apos;t complete: {signInMsg}
              </p>
            )}
            <TestAgain onTest={onTest} />
          </div>
        )
      }
      return (
        <div className="flex flex-col" style={{ gap: 9 }}>
          <div className="flex items-start" style={{ gap: 9 }}>
            <Dot color="var(--status-error-dot)" />
            <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
              {name} isn&apos;t responding: {phase.message}
            </p>
          </div>
          <TestAgain onTest={onTest} />
        </div>
      )

    case 'ready_untested':
      return (
        <div className="flex flex-col" style={{ gap: 8 }}>
          <div className="flex items-start" style={{ gap: 9 }}>
            <Dot color="var(--color-state-approved)" />
            <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
              {name} is installed. Test the connection to confirm you&apos;re signed in.
            </p>
          </div>
          <TestAgain onTest={onTest} label="Test connection" />
        </div>
      )

    default:
      return (
        <div className="flex flex-col" style={{ gap: 8 }}>
          <div className="flex items-start" style={{ gap: 9 }}>
            <Dot color="var(--t-muted)" />
            <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-title)' }}>
              Couldn&apos;t tell whether {name} is installed. You can still select it - if the CLI is
              there and signed in, it just works.
            </p>
          </div>
          <TestAgain onTest={onTest} label="Test connection" />
        </div>
      )
  }
}

function TestAgain({ onTest, label = 'Test again' }: { onTest: () => void; label?: string }) {
  return (
    <button onClick={onTest} className="self-start font-mono"
      style={{
        fontSize: 10.5, letterSpacing: '.08em', textTransform: 'uppercase',
        color: 'var(--t-title)', border: '1px solid var(--t-ctrl-border)',
        borderRadius: 6, padding: '5px 10px', background: 'var(--t-box)', cursor: 'pointer',
      }}>
      {label}
    </button>
  )
}
