//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The first-run onboarding wizard — the "A · Rail" shell from the Meridian Setup
// design, wired to the real backend. Renders inside the Tauri "setup" window
// (tray.rs::open_wizard_window) and talks to Rust exclusively over the `invoke`
// bridge. Presentation comes from the design (atoms/steps/data); behaviour —
// permission polling, integrations, sign-in, and the AI-provider choice — is all
// live. No fabricated state.

import { useState, useEffect, useCallback } from 'react'
import type { CSSProperties, ReactNode } from 'react'
import { invoke, load, mutate, tauri } from '@/lib/bridge'
import { STEPS, Welcome, Completion } from './steps'
import type { Wiz } from './steps'
import type { NotifState } from './data'
import type { IntegrationsResponse } from '@/lib/api-types'
import type { RuntimeSettings } from '@/lib/settings'
import { DEFAULT_LLM_PROVIDER, type LlmProviderId } from '@/lib/llm-providers'
import { useLlmProviderDetection } from '@/components/LlmProviderPicker'
import { Btn, Check, Kicker } from './atoms'

const SERIF: CSSProperties = { fontFamily: 'var(--font-serif)' }

export default function SetupWizard() {
  const [welcome, setWelcome] = useState(true)
  const [step, setStep] = useState(0)
  const [done, setDone] = useState(false)
  const [err, setErr] = useState('')

  // Step 1 — permissions (live)
  const [perms, setPerms] = useState<Wiz['perms']>({ accessibility: null, screen: null, notifications: null })

  // Step 2 — integrations. The shared <ConnectTrackers> drives the actual
  // connect flows (OAuth + token save); this just holds the live connected-state
  // (get_integrations) so the rail status + completion summary stay accurate.
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)

  // Step 3 — sign in (Clerk email one-time-code — see ./signin.tsx). The
  // widget owns its own form/busy/error state; this just holds the result.
  const [signedInEmail, setSignedInEmail] = useState<string | null>(null)

  // Step 4 — intelligence. The provider the whole prose pipeline obeys. Persisted to
  // settings.json immediately on pick (not batched to Finish): if the user quits the
  // wizard halfway, the choice they made should still be the choice that runs.
  const [provider, setProviderState] = useState<LlmProviderId>(DEFAULT_LLM_PROVIDER)
  // Which custom endpoint, when `provider` is 'custom' — see RuntimeSettings.
  const [providerCustomId, setProviderCustomIdState] = useState<string | null>(null)
  const {
    status: providers, scanning: scanningProviders,
    testingIds: testingProviderIds, testOne: testProvider, rescan: rescanProviders,
  } = useLlmProviderDetection()

  // Seed from settings.json rather than assuming the default — a re-run of the wizard
  // must show what the user actually has, not reset them to the default.
  useEffect(() => {
    load<RuntimeSettings>('/api/settings', 'get_settings')
      .then((s) => {
        if (s?.llm_provider) setProviderState(s.llm_provider)
        setProviderCustomIdState(s?.llm_provider_custom_id ?? null)
      })
      .catch(() => {})
  }, [])

  const setProvider = useCallback((id: LlmProviderId, customId?: string) => {
    const prev = provider
    const prevCustom = providerCustomId
    // 'custom' is a kind, not an endpoint — it only ever travels with the id of the one
    // chosen, which update_settings requires and validates.
    const fields: Partial<RuntimeSettings> =
      id === 'custom' ? { llm_provider: id, llm_provider_custom_id: customId ?? null } : { llm_provider: id }
    setProviderState(id)  // optimistic — the picker must feel instant
    if (id === 'custom') setProviderCustomIdState(customId ?? null)
    mutate<RuntimeSettings>('/api/settings', 'update_settings', fields, 'PUT')
      // Roll back on a rejected write, or the UI would claim a choice the daemon
      // isn't honouring — the exact silent-mismatch update_settings validates against.
      .catch((e) => { setProviderState(prev); setProviderCustomIdState(prevCustom); setErr(String(e)) })
  }, [provider, providerCustomId])

  const active = !welcome && !done

  // Poll the two required permissions + optional notifications on the
  // Permissions step. Input Monitoring is intentionally not polled — it's
  // redundant with Accessibility (see the note on PERMISSIONS in ./data.ts).
  // Notification state also refreshes here after the user answers the OS
  // dialog or flips the toggle in System Settings.
  useEffect(() => {
    if (!active || step !== 0) return
    const poll = async () => {
      const [accessibility, screen, notifications] = await Promise.all([
        invoke<boolean>('check_accessibility').catch(() => false),
        invoke<boolean>('check_screen_recording').catch(() => false),
        invoke<NotifState>('check_notifications').catch((): NotifState => 'unavailable'),
      ])
      setPerms((prev) => ({
        accessibility, screen,
        // Bundled-ness is fixed for the process lifetime, so once we've seen a
        // real state (prompt/granted/denied) a later 'unavailable' can only be
        // a transient Swift-bridge hiccup on this one poll tick, never a
        // genuine regression — keep the last known-good state instead of
        // flashing the card away and back every ~2s.
        notifications:
          notifications === 'unavailable' && prev.notifications && prev.notifications !== 'unavailable'
            ? prev.notifications
            : notifications,
      }))
    }
    poll()
    const id = setInterval(poll, 2000)
    return () => clearInterval(id)
  }, [active, step])

  // Keep the live connected-state fresh while on the Integrations step, so the
  // rail status + completion summary reflect connects made via <ConnectTrackers>
  // (which also calls refetchIntegrations on success). A light poll also catches
  // a browser-OAuth completion the component's own poll already resolved.
  const refetchIntegrations = useCallback(() => {
    load<IntegrationsResponse>('/api/integrations', 'get_integrations')
      .then(setIntegrations)
      .catch(() => {})
  }, [])

  useEffect(() => {
    if (!active || step !== 1) return  // Integrations is now step index 1 (2nd tab)
    refetchIntegrations()
    const id = setInterval(refetchIntegrations, 3000)
    return () => clearInterval(id)
  }, [active, step, refetchIntegrations])

  // ── Actions ────────────────────────────────────────────────────────────────
  const openPane = useCallback((pane: string) => {
    setErr(''); invoke('open_permission_pane', { pane }).catch((e) => setErr(String(e)))
  }, [])

  // Screen Recording needs an explicit request to register the app before the
  // Settings pane shows anything to toggle (else it lists "No Items").
  const grantScreen = useCallback(async () => {
    setErr('')
    try { await invoke('request_screen_recording') } catch { /* prompt is best-effort */ }
    invoke('open_permission_pane', { pane: 'screen_recording' }).catch((e) => setErr(String(e)))
  }, [])

  // Notifications: 'prompt' → request surfaces the one-shot macOS dialog and
  // returns the answer; after a deny macOS never re-prompts, so the only
  // recovery is the System Settings → Notifications pane — go straight there
  // instead of re-requesting (the request would just silently re-resolve to
  // 'denied' with no dialog, adding a pointless round-trip through the
  // notification plugin before ever reaching the pane). The request result
  // updates state immediately; the 2 s poll keeps it honest after pane edits.
  const grantNotifications = useCallback(async (alreadyDenied: boolean) => {
    setErr('')
    if (alreadyDenied) {
      invoke('open_permission_pane', { pane: 'notifications' }).catch((e) => setErr(String(e)))
      return
    }
    try {
      const state = await invoke<NotifState>('request_notifications')
      setPerms((prev) => ({ ...prev, notifications: state }))
      if (state === 'denied') {
        invoke('open_permission_pane', { pane: 'notifications' }).catch((e) => setErr(String(e)))
      }
    } catch (e) { setErr(String(e)) }
  }, [])

  // Persists the Clerk-verified email so the Rust side knows who's signed in
  // even after this webview session ends (see commands::save_account_email).
  // Best-effort: a failed write here never blocks the wizard — the widget
  // already has a live Clerk session and lets the user through regardless.
  const onSignedIn = useCallback((email: string) => {
    setSignedInEmail(email)
    invoke('save_account_email', { email }).catch(() => {})
  }, [])

  const wiz: Wiz = {
    perms, openPane, grantScreen, grantNotifications,
    integrations, refetchIntegrations,
    signedInEmail, onSignedIn,
    provider, providerCustomId, setProvider, providers, scanningProviders,
    testingProviderIds, testProvider, rescanProviders,
  }

  // ── Navigation ───────────────────────────────────────────────────────────────
  const meta = STEPS[step]
  const last = step === STEPS.length - 1
  const goStep = (i: number) => { setErr(''); setWelcome(false); setDone(false); setStep(i) }
  const finish = async () => {
    // `mark_setup_complete` writes the onboarded flag that stops the wizard
    // reopening next launch. Only show "complete" if it actually persisted —
    // otherwise the user would think they're done but the wizard would reappear.
    setErr('')
    try {
      await invoke('mark_setup_complete')
      setDone(true)
    } catch (e) {
      setErr(String(e))
    }
  }
  const closeWindow = async () => {
    try {
      await invoke('open_dashboard')
    } catch { /* ignore if dashboard fails to open */ }
    tauri()?.window.getCurrentWindow().close()
  }

  return (
    <div style={{
      position: 'fixed', inset: 0, display: 'grid', placeItems: 'center',
      // Subtle top-lit depth so the centred card reads as a distinct surface —
      // otherwise, when the window is enlarged / full-screened, the card floats
      // on a big flat panel.
      background: 'radial-gradient(130% 130% at 50% 0%, color-mix(in srgb, var(--t-card) 34%, var(--t-panel)) 0%, var(--t-panel) 62%)',
    }}>
      <div className="rise" style={{
        width: 948, height: 628, borderRadius: 18, background: 'var(--t-card)',
        border: '0.5px solid var(--t-card-border)', overflow: 'hidden', color: 'var(--t-title)',
        boxShadow: 'var(--pop-shadow)',
        // Grow the whole card proportionally as the window grows (macOS
        // full-screen / manual resize) so it stays a prominent, readable surface
        // instead of a small rectangle. clamp floor = 1 (never shrinks below the
        // design size at the default window), cap = 1.7 (won't balloon on a 27").
        // min() of the width- and height-fits guarantees it never exceeds the
        // viewport; vector text scales crisply.
        transform: 'scale(clamp(1, min(calc(100vw / 1010), calc(100vh / 700)), 1.7))',
        transformOrigin: 'center',
      }}>
        {welcome ? (
          <Welcome onBegin={() => { setWelcome(false); setStep(0) }} />
        ) : (
          <div className="flex" style={{ height: '100%' }}>
            <Rail step={step} done={done} wiz={wiz} goStep={goStep} />
            <div className="flex flex-col" style={{ flex: 1, minWidth: 0 }}>
              {done ? (
                <div className="nice-scroll" style={{ flex: 1, overflowY: 'auto', display: 'grid', placeItems: 'center', padding: '28px 32px' }}>
                  <div className="flex flex-col items-center">
                    <Completion wiz={wiz} />
                    <Btn onClick={closeWindow} style={{ marginTop: 22, padding: '10px 24px', fontSize: 13.5 }}>Open Meridian</Btn>
                  </div>
                </div>
              ) : (
                <>
                  <div style={{ padding: '26px 32px 16px' }}>
                    <Kicker style={{ marginBottom: 9 }}>{meta.kicker}</Kicker>
                    <h1 style={{ ...SERIF, fontSize: 27, lineHeight: 1.04, letterSpacing: '-.01em', color: 'var(--t-title)' }}>{meta.title}</h1>
                    <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-muted)', marginTop: 8, maxWidth: 460, textWrap: 'pretty' }}>{meta.subtitle}</p>
                  </div>
                  <div className="nice-scroll" style={{ flex: 1, overflowY: 'auto', padding: '4px 32px 22px' }}>
                    <meta.Body wiz={wiz} />
                  </div>
                  <Footer step={step} last={last} canNext={meta.canNext(wiz)} err={err}
                    onBack={() => { setErr(''); setStep(Math.max(0, step - 1)) }}
                    onNext={() => (last ? finish() : (setErr(''), setStep(step + 1)))} />
                </>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

// ── Left step rail ────────────────────────────────────────────────────────────
function Rail({ step, done, wiz, goStep }: { step: number; done: boolean; wiz: Wiz; goStep: (i: number) => void }) {
  return (
    <div className="flex flex-col" style={{ width: 250, flexShrink: 0, background: 'var(--t-box)', borderRight: '1px solid var(--t-hair)', padding: '22px 18px' }}>
      <div style={{ padding: '0 6px', marginBottom: 26 }}>
        <div className="flex items-center" style={{ gap: 8 }}>
          <span style={{ width: 8, height: 8, borderRadius: 99, background: 'var(--color-state-proposal)' }} />
          <span style={{ ...SERIF, fontSize: 21, lineHeight: 1, letterSpacing: '.01em', color: 'var(--t-title)' }}>meridian</span>
        </div>
      </div>
      <div className="flex flex-col" style={{ gap: 2 }}>
        {STEPS.map((s, i) => {
          const isCur = i === step && !done
          const reached = done || i <= step
          const ok = done || i < step
          // A future step is reachable only once every step between the current
          // one and it satisfies its gate — so the rail can't skip a required
          // step (e.g. permissions) that the Footer's "Continue" would block.
          const reachable = done || i <= step || STEPS.slice(step, i).every((p) => p.canNext(wiz))
          return (
            <button key={s.id} disabled={!reachable} onClick={() => { if (reachable) goStep(i) }} className="flex items-start"
              style={{ gap: 12, padding: '10px 8px', borderRadius: 10, textAlign: 'left',
                cursor: reachable ? 'pointer' : 'not-allowed', opacity: reachable ? 1 : 0.55,
                background: isCur ? 'color-mix(in srgb, var(--color-state-proposal) 8%, transparent)' : 'transparent', transition: 'background .14s' }}
              onMouseEnter={(e) => { if (!isCur && reachable) e.currentTarget.style.background = 'var(--t-card)' }}
              onMouseLeave={(e) => { if (!isCur) e.currentTarget.style.background = 'transparent' }}>
              <span className="flex items-center justify-center font-mono shrink-0" style={{
                width: 24, height: 24, borderRadius: 99, fontSize: 11, fontWeight: 600, marginTop: 1,
                background: ok ? 'var(--color-state-proposal)' : isCur ? 'var(--t-card)' : 'transparent',
                color: ok ? '#fff' : isCur ? 'var(--color-state-proposal)' : 'var(--t-faint-2)',
                border: ok ? 'none' : `1px solid ${isCur ? 'var(--color-state-proposal)' : 'var(--t-card-border)'}`,
              }}>{ok ? <Check size={13} color="#fff" /> : s.n}</span>
              <div style={{ minWidth: 0, paddingTop: 1 }}>
                <p style={{ fontSize: 13, fontWeight: isCur ? 500 : 400, color: reached ? 'var(--t-title)' : 'var(--t-faint)' }}>{s.label}</p>
                <p className="font-mono" style={{ fontSize: 10, color: ok ? 'var(--color-state-approved)' : 'var(--t-faint-2)', marginTop: 2, letterSpacing: '.02em' }}>{s.status(wiz)}</p>
              </div>
            </button>
          )
        })}
      </div>
      <div style={{ flex: 1 }} />
      <p className="font-mono" style={{ fontSize: 10, letterSpacing: '.12em', color: 'var(--t-faint-2)', padding: '0 8px', textTransform: 'uppercase' }}>First-run setup</p>
    </div>
  )
}

// ── Footer ────────────────────────────────────────────────────────────────────
function Footer({ step, last, canNext, err, onBack, onNext }: {
  step: number; last: boolean; canNext: boolean; err: string; onBack: () => void; onNext: () => void
}) {
  return (
    <div className="flex items-center justify-between" style={{ padding: '16px 28px', borderTop: '1px solid var(--t-hair)', background: 'var(--t-box)' }}>
      <Btn variant="ghost" disabled={step === 0} onClick={onBack}><ArrowL />Back</Btn>
      <span style={{ fontSize: 11, color: 'var(--color-state-pending)', flex: 1, textAlign: 'center', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', padding: '0 12px' }}>{err}</span>
      <Btn variant="primary" disabled={!canNext} onClick={onNext}>
        {last ? 'Finish setup' : 'Continue'}{!last && <ArrowR />}
      </Btn>
    </div>
  )
}

const ArrowL = (): ReactNode => (<svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="M10 4 6 8l4 4" /></svg>)
const ArrowR = (): ReactNode => (<svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="M6 4l4 4-4 4" /></svg>)
