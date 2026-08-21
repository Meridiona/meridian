//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The per-provider DETAIL view — what opens when a provider tile is clicked in the chooser.
// One provider, one screen: install its CLI if it's missing, confirm the sign-in if it's
// there, and make it the default (the model always follows the provider's own default -
// there is deliberately no model control). The three coding-agent CLIs run on a subscription
// the user already has, so there is no API key to paste; the custom (bring-your-own-key) case
// has its own flow and is NOT rendered here (see <LlmProviderPicker>'s custom detail).
//
// ── Layout contract ──────────────────────────────────────────────────────────────────────
// FOUR bands, in the order a first-time user needs them, and nothing else:
//
//   1. IDENTITY   who this is + one pill saying where it stands right now
//   2. STEP       the single next action, as a titled card - never more than one primary button
//   3. FOOTNOTES  usage + privacy, demoted into one strip so they cannot compete with (2)
//   4. COMMIT     "Use <provider>", or the confirmation that it IS the one running
//
// Type scale is deliberately FOUR sizes (`T` below is the only place they are written down),
// because the version this replaced used six and every paragraph on it therefore weighed the
// same.
//
// ── The claim this screen is allowed to make ─────────────────────────────────────────────
// "<provider> is writing your summaries" and the IN USE pill are gated on `active` — selected
// AND a real passing test — NOT on `selected` alone, which only means a string was written to
// settings.json and is true of the default provider before anyone has touched anything.
// Reading it as "working" put a green "is writing your summaries" banner directly beneath a
// panel saying the CLI was not signed in.

import { BackLink } from '@/components/ui/BackLink'
import { useEffect, useRef, useState } from 'react'
import { openExternal } from '@/lib/bridge'
import { ProviderLogo } from '@/components/LlmProviderLogos'
import { USAGE_FOOTPRINT_NOTE, llmSignIn, type LlmProviderMeta } from '@/lib/llm-providers'
import type { InstallOutcome, ProviderStatus, ProviderTestResult } from '@/components/LlmProviderPicker'

/** The four type sizes this screen is allowed to use. */
const T = {
  name: { fontSize: 18, fontWeight: 700, letterSpacing: '-0.01em', color: 'var(--t-title)' },
  step: { fontSize: 13.5, fontWeight: 650, color: 'var(--t-title)' },
  body: { fontSize: 12.5, lineHeight: 1.55, color: 'var(--t-muted)' },
  hint: { fontSize: 11, lineHeight: 1.5, color: 'var(--t-faint)' },
} as const

/** The connection state we render, derived from install + last-test + in-flight flags. */
export type Phase =
  | { kind: 'installing' } | { kind: 'signing' } | { kind: 'testing' }
  | { kind: 'not_installed' } | { kind: 'ready_untested' } | { kind: 'unknown' }
  | { kind: 'ok' }
  | { kind: 'failed'; message: string }
  | { kind: 'rate_limited'; message: string }

/** Exported for `__tests__/llm-provider-connection-phase.test.ts` — pure decision logic, no
 *  render harness needed (see that suite's header for why). */
export function phaseFor(
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

/** Phases where committing this provider would write a choice we KNOW does not work.
 *  `unknown`/`ready_untested` are deliberately absent: an un-probed CLI may well be installed
 *  and signed in, and refusing it would strand anyone whose probe failed for an unrelated
 *  reason. Exported for the test suite. */
export const BLOCKS_USE: Phase['kind'][] = ['not_installed', 'failed', 'installing', 'signing', 'testing']

/** The one pill in the header, saying where this provider stands. Exported so the wording
 *  can be asserted without a render harness. */
export function statusPill(phase: Phase, active: boolean): { label: string; tone: string } {
  const green = 'var(--color-state-approved)', amber = 'var(--color-state-pending)', grey = 'var(--t-faint)'
  if (active) return { label: 'IN USE', tone: green }
  switch (phase.kind) {
    case 'ok': return { label: 'CONNECTED', tone: green }
    case 'installing': return { label: 'INSTALLING', tone: grey }
    case 'signing': return { label: 'SIGNING IN', tone: grey }
    case 'testing': return { label: 'CHECKING', tone: grey }
    case 'not_installed': return { label: 'NOT INSTALLED', tone: amber }
    case 'failed': return { label: 'ACTION NEEDED', tone: amber }
    case 'rate_limited': return { label: 'RATE LIMITED', tone: amber }
    case 'ready_untested': return { label: 'INSTALLED', tone: grey }
    default: return { label: 'NOT CHECKED', tone: grey }
  }
}

function Pill({ label, tone }: { label: string; tone: string }) {
  return (
    <span className="font-mono shrink-0" style={{
      fontSize: 9.5, fontWeight: 700, letterSpacing: '.09em', color: tone,
      border: `1px solid ${tone}`, background: `color-mix(in srgb, ${tone} 10%, transparent)`,
      borderRadius: 5, padding: '2.5px 7px', whiteSpace: 'nowrap',
    }}>{label}</span>
  )
}

function Check() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2"
      strokeLinecap="round" strokeLinejoin="round" className="shrink-0" aria-hidden="true"><path d="M13 4 6 11 3 8" /></svg>
  )
}

/** The step card's leading glyph — a dot for a waiting state, a tick for a settled one, a
 *  spinner for work in flight. One 22px column so every step's text starts on the same line. */
function StepIcon({ phase }: { phase: Phase }) {
  const k = phase.kind
  const col = { width: 22, marginTop: 1 } as const
  if (k === 'installing' || k === 'signing' || k === 'testing') {
    return (
      <span style={col} className="flex justify-center">
        <span className="inline-block shrink-0" style={{
          width: 15, height: 15, borderRadius: 99,
          border: '2px solid var(--t-ctrl-border)', borderTopColor: 'var(--t-accent)',
          animation: 'spin 0.7s linear infinite',
        }} />
      </span>
    )
  }
  if (k === 'ok') {
    return <span style={{ ...col, color: 'var(--color-state-approved)' }} className="flex justify-center"><Check /></span>
  }
  const tone = k === 'not_installed' || k === 'failed' || k === 'rate_limited'
    ? 'var(--color-state-pending)' : 'var(--t-faint)'
  return (
    <span style={col} className="flex justify-center">
      <span style={{ width: 8, height: 8, borderRadius: 99, background: tone, marginTop: 4 }} />
    </span>
  )
}

/** The step card's primary button — exactly one per phase, ever. */
function StepButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button onClick={onClick} className="self-start" style={{
      fontSize: 12.5, fontWeight: 650, padding: '8px 16px', borderRadius: 9, border: 'none',
      background: 'var(--btn-primary-bg)', color: '#fff', cursor: 'pointer',
    }}>{label}</button>
  )
}

function TestAgain({ onTest, label = 'Test again' }: { onTest: () => void; label?: string }) {
  return (
    <button onClick={onTest} className="self-start font-mono" style={{
      fontSize: 10, letterSpacing: '.08em', textTransform: 'uppercase', fontWeight: 700,
      color: 'var(--t-muted)', border: '1px solid var(--t-ctrl-border)',
      borderRadius: 7, padding: '6px 11px', background: 'var(--t-box)', cursor: 'pointer',
    }}>{label}</button>
  )
}

/** Offered ONLY on a failure - fixing this provider is never the only way forward. Plain
 *  text-link weight (quieter than `TestAgain`'s bordered box) because retrying the SAME
 *  provider is the more likely next move; this is the escape hatch, not a peer action. */
function SwitchProvider({ onClick }: { onClick: () => void }) {
  return (
    <button onClick={onClick} className="self-start underline" style={{
      ...T.hint, color: 'var(--t-accent)', background: 'none', border: 'none',
      padding: 0, cursor: 'pointer', textAlign: 'left',
    }}>
      Try a different provider
    </button>
  )
}

const Mono = ({ children }: { children: React.ReactNode }) =>
  <code style={{ fontFamily: 'var(--font-mono, ui-monospace)', fontSize: '0.94em' }}>{children}</code>

/** One row of the demoted footnote strip: a mono label in a fixed column, then the text. */
function Foot({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline" style={{ gap: 11 }}>
      <span className="font-mono shrink-0" style={{
        fontSize: 9, fontWeight: 700, letterSpacing: '.1em', color: 'var(--t-faint-2)',
        width: 54, textAlign: 'right',
      }}>{label}</span>
      <p style={T.hint}>{children}</p>
    </div>
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
  /** Run the interactive browser sign-in - OAuth against the user's own subscription. Covers
   *  every id in `LLM_SIGN_IN` (`@/lib/llm-providers`); deliberately not enumerated here,
   *  since that list is what drifted. No-op for providers without an in-app sign-in. */
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

  // Saved AND proven. See the header note: `selected` on its own is just "this string is in
  // settings.json", which is true of the default provider before the user has done anything.
  const active = selected && phase.kind === 'ok'
  const blocked = BLOCKS_USE.includes(phase.kind)
  const pill = statusPill(phase, active)

  // Auto-test once when we open on an installed CLI with no recent result. Guarded so a status
  // refresh can't re-fire it, and skipped when a test ran in the last minute so flipping back
  // and forth doesn't burn requests.
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
    // On success the hook re-detects and auto-tests, so the panel moves on by itself.
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
    <div className="flex flex-col" style={{ gap: 18, maxWidth: 560 }}>
      {/* Back to the chooser */}
      <BackLink onClick={onBack}>All providers</BackLink>

      {/* 1 — IDENTITY. No RECOMMENDED badge: every tile in the chooser carries it, so on the
          screen you reach BY clicking one it says nothing and only crowds the pill that does. */}
      <div className="flex items-center" style={{ gap: 14 }}>
        <span className="flex items-center justify-center shrink-0" style={{
          width: 52, height: 52, borderRadius: 14,
          background: active ? 'color-mix(in srgb, var(--color-state-approved) 12%, transparent)' : 'var(--t-box)',
          border: `1px solid ${active ? 'var(--color-state-approved)' : 'var(--t-ctrl-border)'}`,
          transition: 'background 260ms ease, border-color 260ms ease',
        }}>
          <ProviderLogo id={p.id} size={27} />
        </span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div className="flex items-center" style={{ gap: 9 }}>
            <span style={T.name}>{p.name}</span>
            <Pill label={pill.label} tone={pill.tone} />
          </div>
          <p style={{ ...T.body, marginTop: 4 }}>{p.blurb}</p>
        </div>
      </div>

      {/* 2 — STEP. One card, one heading, at most one primary button. */}
      <div className="flex" style={{
        gap: 11, padding: '15px 17px 16px', borderRadius: 13,
        background: 'var(--t-card)', border: '1px solid var(--t-ctrl-border)',
      }}>
        <StepIcon phase={phase} />
        <div className="flex flex-col" style={{ gap: 9, flex: 1, minWidth: 0 }}>
          {/* installHint: the probe reports the command THIS platform actually runs. The static
              meta hint is a single string compiled for both platforms and names the npm command,
              which is wrong on Windows - and it doubles as the "run it yourself" fallback after a
              failed install. Fall back to it only while the probe is still in flight. */}
          <ConnectionBody
            phase={phase}
            name={p.name}
            providerId={p.id}
            installHint={probed?.install_command ?? p.installHint}
            installMsg={installMsg}
            signInMsg={signInMsg}
            onInstall={install}
            onSignIn={signIn}
            onTest={onTest}
            onSwitchProvider={onBack}
          />
        </div>
      </div>

      {/* 3 — FOOTNOTES. The two facts a careful user wants before pointing a paid plan at
          this, demoted into one labelled strip - loose in the flow they read as two more
          paragraphs of equal weight to the step above. */}
      <div className="flex flex-col" style={{
        gap: 8, padding: '12px 14px', borderRadius: 11,
        background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
      }}>
        <Foot label="USAGE">{USAGE_FOOTPRINT_NOTE}</Foot>
        {/* Honest: we disable telemetry per call, but opting out of TRAINING is a one-time
            account setting we cannot flip for them. */}
        {p.privacyUrl && (
          <Foot label="PRIVACY">
            Meridian turns off usage telemetry on every call. To also keep your prompts out of
            model training, switch on {p.name}&apos;s privacy setting once:{' '}
            <button onClick={() => p.privacyUrl && openExternal(p.privacyUrl)}
              style={{ color: 'var(--t-accent)', background: 'none', border: 'none', padding: 0, cursor: 'pointer', font: 'inherit', fontWeight: 600, textDecoration: 'underline' }}>
              {p.privacyLabel ?? 'open settings'} ↗
            </button>
          </Foot>
        )}
      </div>

      {/* 4 — COMMIT. */}
      <div className="flex flex-col" style={{ gap: 7 }}>
        {applyErr && <p style={{ ...T.hint, color: 'var(--status-error-dot)' }}>{applyErr}</p>}
        {active ? (
          <div className="flex items-center self-start" style={{
            gap: 8, fontSize: 13, fontWeight: 600, color: 'var(--color-state-approved)',
            padding: '10px 16px', borderRadius: 10,
            background: 'color-mix(in srgb, var(--color-state-approved) 10%, transparent)',
            border: '1px solid var(--color-state-approved)',
          }}>
            <Check />
            {p.name} is writing your summaries
          </div>
        ) : (
          <>
            {/* Disabled while the step above is unfinished: committing a provider we just
                PROVED cannot answer writes a setting whose only effect is that the hourly
                summaries silently never run. */}
            <button onClick={use} disabled={applying || blocked} className="self-start"
              style={{
                fontSize: 13, fontWeight: 650, padding: '10px 20px', borderRadius: 10, border: 'none',
                background: applying || blocked ? 'var(--t-ctrl)' : 'var(--btn-primary-bg)',
                color: applying || blocked ? 'var(--t-faint)' : '#fff',
                cursor: applying || blocked ? 'default' : 'pointer',
                transition: 'background 200ms ease, color 200ms ease',
              }}>
              {applying ? 'Switching…' : `Use ${p.name}`}
            </button>
            {blocked && (
              <p style={T.hint}>Finish the step above and this turns on.</p>
            )}
          </>
        )}
      </div>
    </div>
  )
}

/** The body of the step card — one heading, one explanation, at most one primary button. */
function ConnectionBody({
  phase, name, providerId, installHint, installMsg, signInMsg, onInstall, onSignIn, onTest,
  onSwitchProvider,
}: {
  phase: Phase; name: string; providerId: string
  installHint?: string; installMsg: string | null; signInMsg: string | null
  onInstall: () => void; onSignIn: () => void; onTest: () => void
  /** Back to the chooser grid - offered on a failure so getting THIS provider working is
   *  never the only way forward. Same destination `<BackLink>` already reaches from the
   *  top of the screen; this just puts it one click closer to where the failure is read,
   *  matching `<AiEngineNotice>`'s "Check your provider" pattern on the dashboard's own
   *  Draft-with-AI failure path. */
  onSwitchProvider: () => void
}) {
  // Providers whose CLI authenticates against the user's OWN subscription via a browser OAuth
  // Meridian can drive in-app. From the shared registry in `@/lib/llm-providers`, so the copy
  // here and the tray command the picker invokes cannot desync - they used to be two hard-coded
  // lists. `null` for providers with no in-app sign-in (e.g. Copilot, or a cloud endpoint).
  const signInProvider = llmSignIn(providerId)

  switch (phase.kind) {
    case 'installing':
      return (
        <>
          <span style={T.step}>Installing the {name} CLI</span>
          <p style={T.body}>This usually takes about a minute. You can leave it running.</p>
        </>
      )

    case 'signing':
      return (
        <>
          <span style={T.step}>Waiting for your browser</span>
          <p style={T.body}>
            Finish signing in to {signInProvider?.account ?? name} in the tab that just opened -
            this updates on its own.
          </p>
        </>
      )

    case 'not_installed':
      return (
        <>
          <span style={T.step}>Install the {name} CLI</span>
          <p style={T.body}>
            It isn&apos;t on this Mac yet. Meridian can install it for you - one click, nothing to
            configure.
          </p>
          <StepButton label={`Install ${name}`} onClick={onInstall} />
          {installHint && !installMsg && (
            <p style={T.hint}>Runs <Mono>{installHint}</Mono> in your shell.</p>
          )}
          {installMsg && (
            <div className="flex flex-col" style={{ gap: 6 }}>
              <p style={{ ...T.body, color: 'var(--status-error-dot)' }}>That didn&apos;t work: {installMsg}</p>
              {installHint && (
                <p style={{ ...T.hint, color: 'var(--t-key-text)', background: 'var(--t-key-bg)', borderRadius: 7, padding: '7px 10px' }}>
                  <Mono>{installHint}</Mono>
                </p>
              )}
              <p style={T.hint}>Run that in your terminal, then come back and reopen this.</p>
            </div>
          )}
        </>
      )

    case 'testing':
      return (
        <>
          <span style={T.step}>Checking your {name} sign-in</span>
          <p style={T.body}>One quick request, to make sure it answers.</p>
        </>
      )

    case 'ok':
      return (
        <>
          <span style={T.step}>Connected and ready</span>
          <p style={T.body}>You&apos;re signed in to {name} and it answered. Nothing else to set up.</p>
        </>
      )

    case 'rate_limited':
      return (
        <>
          <span style={T.step}>Signed in, but rate-limited right now</span>
          <p style={T.body}>{phase.message}. It picks back up on its own once the limit clears.</p>
          <TestAgain onTest={onTest} />
        </>
      )

    case 'failed':
      // For providers signing into the user's own subscription (Cursor, Codex, Claude), a
      // "failure" is almost always just "not signed in yet" - a one-time browser OAuth, no API
      // key. Offered as a one-click in-app sign-in, with the terminal as a fallback.
      if (signInProvider) {
        return (
          <>
            <span style={T.step}>Sign in to {signInProvider.account}</span>
            <p style={T.body}>
              The CLI is installed but not signed in yet. Sign in once and Meridian runs on your
              {' '}{signInProvider.subscription} - no API key, no extra cost.
            </p>
            <StepButton label={signInProvider.label} onClick={onSignIn} />
            {signInMsg && (
              <p style={{ ...T.body, color: 'var(--status-error-dot)' }}>
                Sign-in didn&apos;t complete: {signInMsg}
              </p>
            )}
            <div className="flex items-center flex-wrap" style={{ gap: 10 }}>
              <TestAgain onTest={onTest} />
              <p style={{ ...T.hint, flex: 1, minWidth: 180 }}>
                Prefer the terminal? Run <Mono>{signInProvider.cmd}</Mono>, then test again.
              </p>
            </div>
            <SwitchProvider onClick={onSwitchProvider} />
          </>
        )
      }
      return (
        <>
          <span style={T.step}>{name} isn&apos;t responding</span>
          <p style={T.body}>{phase.message}</p>
          <TestAgain onTest={onTest} />
          <SwitchProvider onClick={onSwitchProvider} />
        </>
      )

    case 'ready_untested':
      return (
        <>
          <span style={T.step}>{name} is installed</span>
          <p style={T.body}>Test the connection to confirm you&apos;re signed in to it.</p>
          <TestAgain onTest={onTest} label="Test connection" />
        </>
      )

    default:
      return (
        <>
          <span style={T.step}>Couldn&apos;t tell whether {name} is installed</span>
          <p style={T.body}>
            You can still choose it - if the CLI is there and signed in, it just works.
          </p>
          <TestAgain onTest={onTest} label="Test connection" />
        </>
      )
  }
}
