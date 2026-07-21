//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Wizard step bodies + Welcome + Completion + the STEPS meta (ported from the
// design's steps.jsx). Every body is driven by the live `Wiz` handle built in
// page.tsx — permissions, integrations, sign-in, and the AI-provider choice are
// all real. Nothing here fabricates data.

import type { ReactNode } from 'react'
import { Btn, Check, DISPLAY, Kicker, PermIcon, Row } from './atoms'
import { PERMISSIONS } from './data'
import type { NotifState } from './data'
import type { IntegrationsResponse } from '@/lib/api-types'
import { TRACKERS } from '@/lib/integrations'
import { llmProvider, LLM_INTRO_BODY, LLM_INTRO_TITLE, type LlmProviderId } from '@/lib/llm-providers'
import ConnectTrackers from '@/components/IntegrationConnect'
import LlmProviderPicker, { type InstallOutcome, type ProviderStatus } from '@/components/LlmProviderPicker'
import { SignInWidget } from './signin'

/** The live wizard handle page.tsx builds and threads to every step body. */
export interface Wiz {
  // Step 1 — permissions (live, polled every 2 s). The two TCC grants are
  // booleans; notifications is tri-state (see `NotifState`) because deny and
  // not-yet-asked need different grant actions.
  perms: { accessibility: boolean | null; screen: boolean | null; notifications: NotifState | null }
  openPane: (pane: string) => void
  grantScreen: () => void
  grantNotifications: (alreadyDenied: boolean) => void
  // Step 2 — integrations (live connected-state from get_integrations)
  integrations: IntegrationsResponse | null
  refetchIntegrations: () => void
  // Step 3 — sign in (Clerk email one-time-code — see ui/app/setup/signin.tsx).
  // The step body owns its own form/busy/error state; `onSignedIn` is just
  // how it reports a completed sign-in back up to the wizard.
  signedInEmail: string | null
  onSignedIn: (email: string) => void
  // Step 4 — intelligence. The one AI choice everything downstream obeys
  // (settings.json `llm_provider` → src/llm/resolver.rs). `providers` is the live
  // which-CLIs-are-installed probe; it never gates the choice, only informs it.
  provider: LlmProviderId
  /** Which custom endpoint, when `provider` is 'custom'. */
  providerCustomId: string | null
  /** Resolves when the choice is persisted, rejects if it wasn't - the shared detail view
   *  awaits this to render "Switching…" and surface a failure. */
  setProvider: (id: LlmProviderId, customId?: string) => Promise<void>
  providers: Record<string, ProviderStatus>
  scanningProviders: boolean
  testingProviderIds: Set<string>
  installingProviderIds: Set<string>
  signingProviderIds: Set<string>
  testProvider: (id: string) => void
  installProvider: (id: string) => Promise<InstallOutcome>
  signInProvider: (id: string) => Promise<InstallOutcome>
  rescanProviders: () => void
}

// ── STEP 1 — Permissions ──────────────────────────────────────────────────────
function PermissionsBody({ wiz }: { wiz: Wiz }) {
  return (
    <div className="flex flex-col" style={{ gap: 9 }}>
      {PERMISSIONS.map((p) => {
        const notif = p.id === 'notifications'
        // Unbundled runs (`tauri dev`) have no notification plugin at all —
        // nothing to grant, so the card hides rather than dead-ends.
        if (notif && wiz.perms.notifications === 'unavailable') return null
        const granted = notif ? wiz.perms.notifications === 'granted' : !!wiz.perms[p.id]
        return (
          <Row key={p.id} tone={granted ? 'tint' : 'surface'}>
            <span className="flex items-center justify-center shrink-0" style={{
              width: 34, height: 34, borderRadius: 10,
              background: granted ? 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' : 'var(--t-box)',
              color: granted ? 'var(--color-state-proposal)' : 'var(--t-faint)',
              border: '0.5px solid var(--t-card-border)',
            }}><PermIcon icon={p.icon} /></span>

            <div style={{ flex: 1, minWidth: 0 }}>
              <div className="flex items-center" style={{ gap: 8 }}>
                <span style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--t-title)' }}>{p.name}</span>
                {!notif && <span className="mt-chip" style={{ color: 'var(--t-muted)', border: '0.5px solid var(--t-card-border)', borderRadius: 4, padding: '1px 5px' }}>{p.required ? 'REQUIRED' : 'OPTIONAL'}</span>}
              </div>
              <p style={{ fontSize: 11.5, lineHeight: 1.4, color: 'var(--t-muted)', marginTop: 3 }}>{p.desc}</p>
            </div>

            <div className="shrink-0">
              {granted
                ? <span className="flex items-center" style={{ gap: 6, fontSize: 12, color: 'var(--color-state-approved)', fontWeight: 500 }}><Check size={15} color="var(--color-state-approved)" />Granted</span>
                : notif
                  // 'prompt' → the button surfaces the one-shot OS dialog;
                  // 'denied' → macOS won't re-prompt, so it opens the
                  // Notifications pane directly (grantNotifications skips the
                  // pointless re-request when told it's already denied).
                  ? <Btn size="sm" variant="secondary" onClick={() => wiz.grantNotifications(wiz.perms.notifications === 'denied')}>{wiz.perms.notifications === 'denied' ? 'Open Settings' : 'Allow'}</Btn>
                  : <Btn size="sm" variant="secondary" onClick={() => p.id === 'screen' ? wiz.grantScreen() : wiz.openPane(p.pane)}>Open Settings</Btn>}
            </div>
          </Row>
        )
      })}
      <p className="flex items-start" style={{ gap: 7, fontSize: 11, lineHeight: 1.5, color: 'var(--t-muted)', marginTop: 3 }}>
        <span style={{ width: 5, height: 5, borderRadius: 99, background: 'var(--color-state-approved)', marginTop: 5, flexShrink: 0 }} />
        Your screen, tasks, and worklogs stay on this Mac and are never uploaded. We send usage stats - daily focus time, app version, and your email once you sign in - to improve Meridian, never your content.
      </p>
    </div>
  )
}

// ── STEP 2 — Integrations ─────────────────────────────────────────────────────
// The whole connect surface is the shared <ConnectTrackers> (same component the
// dashboard uses), so all 5 providers + every connect flow live in one place.
function IntegrationsBody({ wiz }: { wiz: Wiz }) {
  const connected = TRACKERS.filter((t) => wiz.integrations?.[t.id]).length
  return (
    <div className="flex flex-col" style={{ gap: 9 }}>
      <ConnectTrackers integrations={wiz.integrations} onChanged={wiz.refetchIntegrations} compact />
      <p className="flex items-start" style={{ gap: 7, fontSize: 11, lineHeight: 1.5, color: 'var(--t-muted)', marginTop: 3 }}>
        <span style={{ width: 5, height: 5, borderRadius: 99, background: connected ? 'var(--color-state-approved)' : 'var(--t-faint-2)', marginTop: 5, flexShrink: 0 }} />
        {connected > 0
          ? `${connected} connected · Meridian will match sessions and draft worklogs.`
          : 'Optional - skip it and Meridian still tracks your day. Connect a tracker anytime from Settings to auto-draft worklogs.'}
      </p>
    </div>
  )
}

// ── STEP 3 — Sign in ──────────────────────────────────────────────────────────
// Clerk email one-time-code, entirely inside the webview via tauri-plugin-clerk
// (see lib.rs's plugin registration + signin.tsx). <SignInWidget> is a thin
// next/dynamic boundary around the actual Clerk-aware form (the signin/ module —
// see SignInWidget.tsx/EmailCodeForm.tsx) — importing it here stays safe for the
// static-export build because signin.tsx itself has no @clerk/react imports at
// the top level.
function SignInBody({ wiz }: { wiz: Wiz }) {
  if (wiz.signedInEmail) {
    return (
      <Row tone="tint">
        <span className="flex items-center justify-center shrink-0" style={{
          width: 34, height: 34, borderRadius: 10,
          background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)',
          color: 'var(--color-state-proposal)', border: '0.5px solid var(--t-card-border)',
        }}><Check size={16} color="var(--color-state-proposal)" w={2.2} /></span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <span style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--t-title)' }}>Signed in</span>
          <p style={{ fontSize: 11.5, lineHeight: 1.4, color: 'var(--t-muted)', marginTop: 3 }}>{wiz.signedInEmail}</p>
        </div>
      </Row>
    )
  }
  return <SignInWidget onSignedIn={wiz.onSignedIn} />
}

// ── STEP 4 — Intelligence (which AI writes the summaries) ────────────────────
// One choice, one place. Everything downstream reads it from settings.json — the
// resolver re-reads it on every call (src/llm/resolver.rs), so changing it later in
// Settings takes effect on the next hour with nothing to restart.
//
// The whole surface is the shared <LlmProviderPicker>: a chooser of the three recommended
// coding agents (+ bring-your-own-key), each opening a detail view that installs the CLI,
// confirms the sign-in, and sets the provider as default. The choice is never blocked on the
// CLI being installed — the detail view walks the user through getting it there instead.
function IntelligenceBody({ wiz }: { wiz: Wiz }) {
  return (
    <LlmProviderPicker
      value={wiz.provider}
      selectedCustomId={wiz.providerCustomId}
      onChange={wiz.setProvider}
      status={wiz.providers}
      scanning={wiz.scanningProviders}
      testingIds={wiz.testingProviderIds}
      installingIds={wiz.installingProviderIds}
      signingIds={wiz.signingProviderIds}
      testOne={wiz.testProvider}
      install={wiz.installProvider}
      signIn={wiz.signInProvider}
      rescan={wiz.rescanProviders}
    />
  )
}

// ── Welcome (pre-step intro) ──────────────────────────────────────────────────
export function Welcome({ onBegin, steps }: { onBegin: () => void; steps: StepMeta[] }) {
  const points = [
    { t: 'On-device', d: 'Your screen is read and understood locally on your device, never uploaded.' },
    { t: 'Automatic', d: 'Builds an accurate timeline of the tickets you worked on, then drafts the updates for you.' },
    { t: 'Connected', d: 'Works with Jira, Linear, GitHub, Trello, and Azure DevOps.' },
  ]
  return (
    <div className="flex flex-col items-center justify-center" style={{ height: '100%', textAlign: 'center', padding: '36px 44px' }}>
      <div className="flex items-center mer-pop" style={{ gap: 9, marginBottom: 22 }}>
        <span style={{ width: 9, height: 9, borderRadius: 99, background: 'var(--color-state-proposal)' }} />
        <span style={{ fontFamily: 'var(--font-sans)', fontWeight: 600, fontSize: 19, lineHeight: 1, letterSpacing: '-.01em', color: 'var(--t-title)' }}>meridian</span>
      </div>
      <Kicker style={{ marginBottom: 14 }}>First-run setup</Kicker>
      <h1 style={{ ...DISPLAY, fontSize: 33, lineHeight: 1.08, color: 'var(--t-title)', maxWidth: 400, textWrap: 'balance' }}>
        Your work, <span style={{ color: 'var(--color-state-proposal)' }}>remembered accurately.</span>
      </h1>
      <p style={{ fontSize: 13.5, lineHeight: 1.55, color: 'var(--t-muted)', marginTop: 13, maxWidth: 384, textWrap: 'pretty' }}>
        Meridian watches your work on-device and keeps an accurate record of what you actually did, then turns it into worklogs and ticket updates you just approve.
      </p>
      <div className="flex flex-col" style={{ gap: 11, margin: '26px 0 28px', textAlign: 'left', width: '100%', maxWidth: 360 }}>
        {points.map((p) => (
          <div key={p.t} className="flex items-start" style={{ gap: 11 }}>
            <span className="flex items-center justify-center shrink-0" style={{ width: 19, height: 19, borderRadius: 99, background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)', marginTop: 1 }}>
              <Check size={12} color="var(--color-state-proposal)" w={2.2} />
            </span>
            <p style={{ fontSize: 12.5, lineHeight: 1.4, color: 'var(--t-muted)' }}>
              <span style={{ fontWeight: 500, color: 'var(--t-title)' }}>{p.t}.</span> {p.d}
            </p>
          </div>
        ))}
      </div>
      <Btn onClick={onBegin} style={{ padding: '11px 26px', fontSize: 13.5 }}>Get started</Btn>
      <p className="font-mono" style={{ fontSize: 10.5, letterSpacing: '.04em', color: 'var(--t-faint)', marginTop: 14 }}>{steps.length} quick steps · about a minute</p>
    </div>
  )
}

// ── Completion ────────────────────────────────────────────────────────────────
export function Completion({ wiz }: { wiz: Wiz }) {
  const connected = TRACKERS.filter((t) => wiz.integrations?.[t.id])
  const grantedCount = [wiz.perms.accessibility, wiz.perms.screen].filter(Boolean).length
  const lines = [
    { k: 'Permissions', v: `${grantedCount} of 2 granted` },
    { k: 'Intelligence', v: llmProvider(wiz.provider).name },
    { k: 'Connected', v: connected.length ? connected.map((c) => c.name).join(', ') : 'None yet' },
  ]
  return (
    <div className="flex flex-col items-center" style={{ textAlign: 'center', padding: '8px 8px 0' }}>
      <span className="flex items-center justify-center mer-pop" style={{ width: 56, height: 56, borderRadius: 99, background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)', color: 'var(--color-state-proposal)', marginBottom: 18 }}>
        <Check size={28} color="var(--color-state-proposal)" w={2.2} />
      </span>
      <Kicker style={{ marginBottom: 10 }}>Setup complete</Kicker>
      <h1 style={{ ...DISPLAY, fontSize: 31, lineHeight: 1.05, color: 'var(--t-title)', marginBottom: 10 }}>You&apos;re all set.</h1>
      <p style={{ fontSize: 13.5, lineHeight: 1.5, color: 'var(--t-muted)', maxWidth: 340, textWrap: 'pretty', marginBottom: 22 }}>
        Meridian is now tracking quietly in your menu bar - on-device, private, and matched to your work.
      </p>
      <div style={{ width: '100%', maxWidth: 360, border: '0.5px solid var(--t-card-border)', borderRadius: 13, overflow: 'hidden' }}>
        {lines.map((l, i) => (
          <div key={l.k} className="flex items-center justify-between" style={{ padding: '10px 14px', borderTop: i ? '1px solid var(--t-hair)' : 'none' }}>
            <span className="mt-chip" style={{ color: 'var(--t-faint)' }}>{l.k}</span>
            <span style={{ fontSize: 12.5, color: 'var(--t-title)', fontWeight: 450 }}>{l.v}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

// ── STEP META — order, labels, headers, gating, rail status ───────────────────
export interface StepMeta {
  id: string
  n: string
  label: string
  kicker: string
  title: string
  subtitle: string
  Body: (props: { wiz: Wiz }) => ReactNode
  status: (w: Wiz) => string
  canNext: (w: Wiz) => boolean
}

// Permissions stays first on macOS (capture needs them); the AI-provider
// choice comes last so the user has connected their trackers and signed in
// before picking which model writes their summaries.
//
// Permissions is macOS-only: Windows capture (UIA + WGC) has no TCC-style
// consent system — screenpipe_a11y::platform::windows reports every grant as
// already true — so there is nothing for that step to request, and its copy
// ("Two macOS permissions...") is actively wrong on Windows. `buildSteps`
// drops it entirely there rather than rendering an always-green no-op card.
const ALL_STEPS: StepMeta[] = [
  {
    id: 'permissions', n: '01', label: 'Permissions', kicker: 'Access',
    title: 'Let Meridian see your work',
    subtitle: "Two macOS permissions let Meridian recognise what you're focused on. Read locally, never uploaded.",
    Body: PermissionsBody,
    status: (s) => { const g = [s.perms.accessibility, s.perms.screen].filter(Boolean).length; return g ? `${g} granted` : 'Not granted' },
    canNext: (s) => !!(s.perms.accessibility && s.perms.screen),
  },
  {
    id: 'integrations', n: '02', label: 'Integrations', kicker: 'Project tools',
    title: 'Connect your trackers',
    subtitle: 'Link the tools you use so Meridian can match sessions to tickets and draft worklogs - skip it and Meridian still tracks your day; connect anytime from Settings.',
    Body: IntegrationsBody,
    status: (s) => { const c = TRACKERS.filter((t) => s.integrations?.[t.id]).length; return c ? `${c} connected` : 'Optional' },
    canNext: () => true,
  },
  {
    id: 'signin', n: '03', label: 'Sign in', kicker: 'Account',
    title: 'Sign in to Meridian',
    subtitle: "One quick sign-in so we know who's using Meridian - we'll email you a one-time code, no password needed.",
    Body: SignInBody,
    status: (s) => s.signedInEmail ?? 'Not signed in',
    canNext: (s) => !!s.signedInEmail,
  },
  {
    id: 'provider', n: '04', label: 'Intelligence', kicker: 'Your AI',
    // The SHARED copy - the same words Settings shows. These used to be hardcoded here with
    // different wording while llm-providers.ts claimed they were shared.
    title: LLM_INTRO_TITLE,
    subtitle: LLM_INTRO_BODY,
    Body: IntelligenceBody,
    status: (s) => llmProvider(s.provider).name,
    // Never gates. Every path out of this step is valid — including picking a CLI that
    // isn't installed yet: that hour is left pending rather than dead-ending setup.
    canNext: () => true,
  },
]

/** Platform-specific step list. `n` is renumbered to the filtered position so
 *  the rail never shows a gap (e.g. "02, 03, 04" with only three steps). */
export function buildSteps(platform: string | null): StepMeta[] {
  const steps = platform === 'windows' ? ALL_STEPS.filter((s) => s.id !== 'permissions') : ALL_STEPS
  return steps.map((s, i) => ({ ...s, n: String(i + 1).padStart(2, '0') }))
}
