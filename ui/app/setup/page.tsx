//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The first-run onboarding wizard — the "A · Rail" shell from the Meridian Setup
// design, wired to the real backend. Renders inside the Tauri "setup" window
// (tray.rs::open_wizard_window) and talks to Rust exclusively over the `invoke`
// bridge. Presentation comes from the design (atoms/steps/data); behaviour —
// permission polling, integrations, sign-in, and the AI-provider choice — is all
// live. No fabricated state.

import { useState, useEffect, useLayoutEffect, useMemo, useCallback, useRef } from 'react'
import type { ReactNode } from 'react'
import { invoke, load, mutate, nudgeWindowRepaint, tauri } from '@/lib/bridge'
import { buildSteps, resumeIndex, Welcome } from './steps'
import type { Wiz } from './steps'
import type { NotifState } from './data'
import type { IntegrationsResponse } from '@/lib/api-types'
import type { RuntimeSettings } from '@/lib/settings'
import { DEFAULT_LLM_PROVIDER, providerChoiceFields, type LlmProviderId } from '@/lib/llm-providers'
import { useLlmProviderDetection } from '@/components/LlmProviderPicker'
import { Btn, DISPLAY, Kicker } from './atoms'

/** Where the wizard stores how far the user got, so quitting mid-setup resumes
 *  rather than restarting. Holds a step ID (see the restore effect for why an
 *  index would be wrong), and is cleared on finish so a later Re-run Setup
 *  opens on Welcome instead of resuming a run that is already over.
 *
 *  Versioned in the key, matching `meridian.walkthrough.seen.v1`: the stored
 *  shape is read by a build that may not be the one that wrote it, and a `.v2`
 *  is how that gets changed without having to parse a legacy shape. */
const PROGRESS_KEY = 'meridian.setup.progress.v1'

export default function SetupWizard() {
  const [welcome, setWelcome] = useState(true)
  const [step, setStep] = useState(0)
  const [err, setErr] = useState('')

  // Platform shapes the Permissions step — macOS shows the two TCC grants +
  // notifications, Windows shows notifications only (no TCC analogue there; see
  // buildSteps in ./steps.tsx). The step count is the same on both. `null`
  // until resolved. The Welcome screen (the only way past `welcome`) blocks
  // "Get started" until this resolves, so `steps` can never change shape
  // out from under a `step` index the user already navigated to — a late
  // resolve reshaping the list AFTER the user has begun would otherwise let
  // `step` point at the wrong step or past the end of a now-shorter list.
  const [platform, setPlatform] = useState<string | null>(null)
  const [platformError, setPlatformError] = useState(false)
  const resolvePlatform = useCallback(() => {
    setPlatformError(false)
    invoke<string>('get_platform').then(setPlatform).catch(() => setPlatformError(true))
  }, [])
  useEffect(() => { resolvePlatform() }, [resolvePlatform])

  // ── Notifications precheck — decides whether the Alerts step exists at all ──
  // On Windows the Permissions step is a single optional notifications toggle,
  // and Windows grants toast notifications to most apps by default - so for most
  // users it opened already GRANTED, an onboarding screen asking for something
  // it already had. `buildSteps` drops it when this resolves 'granted'.
  //
  // This is a ONE-SHOT read, deliberately separate from the live 2 s poll below:
  // the poll only runs while the Permissions step is already on screen, which is
  // circular for a question that decides whether that step is on screen at all.
  //
  // Resolved ONCE and never rewritten - `notifPrecheck` is not updated by the
  // poll, and the `.then` cannot fire twice. That is what keeps `steps` stable
  // once the user has begun: the same invariant the `platform` note above
  // describes, and it is why Welcome's "Get started" waits on this too. Someone
  // who grants notifications mid-wizard (via the step itself) keeps the step
  // they are standing on rather than having it vanish underfoot.
  const [notifPrecheck, setNotifPrecheck] = useState<NotifState | null>(null)
  useEffect(() => {
    invoke<NotifState>('check_notifications')
      // A failed probe is NOT a grant: fall through to 'unavailable', which
      // keeps the step (the wizard's existing behaviour) rather than silently
      // skipping notification onboarding because one IPC call went wrong.
      .catch((): NotifState => 'unavailable')
      .then(setNotifPrecheck)
  }, [])

  const steps = useMemo(
    () => buildSteps(platform, notifPrecheck === 'granted'),
    [platform, notifPrecheck],
  )

  // ── Resume where they left off ───────────────────────────────────────────
  // Quitting mid-setup used to drop the user back on Welcome, re-reading a
  // screen they had already read and re-walking steps that were already done
  // (permissions granted, tracker connected - the work survives, only the
  // position in the flow was lost).
  //
  // The step's ID is what gets stored, never its index. `buildSteps` is
  // platform-dependent and the list can change between builds, so an index is
  // only meaningful against the exact list that produced it - restoring `2`
  // could land on a different step entirely, silently. An id this build has
  // never heard of starts from the top; an id it knows but has DROPPED (the
  // Windows Alerts step, once notifications are granted) resumes past it
  // rather than throwing the position away - see `resumeIndex`.
  //
  // Restore is gated on `platform` having resolved, which preserves the
  // invariant the Welcome screen exists to hold (see `platform` above): the
  // step list must never change shape underneath a `step` index that has
  // already been navigated to. Doing this before the platform resolves would
  // reintroduce exactly that bug through the back door.
  const restoredProgress = useRef(false)
  useEffect(() => {
    // Gated on `notifPrecheck` for the same reason as `platform`: it is the
    // other input to `buildSteps`, so restoring a position before it lands
    // could point `step` into a list that is about to lose a step.
    if (restoredProgress.current || platform === null || notifPrecheck === null) return
    restoredProgress.current = true
    let savedStepId: string | null = null
    try {
      const raw = localStorage.getItem(PROGRESS_KEY)
      savedStepId = raw ? (JSON.parse(raw) as { stepId?: string }).stepId ?? null : null
    } catch { return /* private mode, or a hand-mangled value - start fresh */ }
    if (!savedStepId) return
    const at = resumeIndex(steps, savedStepId)
    if (at < 0) return
    setWelcome(false)
    setStep(at)
  }, [platform, notifPrecheck, steps])

  // Written on every move once past Welcome. Welcome itself is deliberately
  // not stored: it is the zero state, and "resuming" onto it is the same as
  // not resuming.
  useEffect(() => {
    if (welcome || platform === null) return
    const stepId = steps[step]?.id
    if (!stepId) return
    try { localStorage.setItem(PROGRESS_KEY, JSON.stringify({ stepId })) } catch { /* private mode */ }
  }, [welcome, step, steps, platform])

  // Stamp when onboarding BEGAN. Completion has always been timestamped
  // (`~/.meridian/onboarded`) but the start was not, so the elapsed time — shown
  // as "Setup complete in Ns" and as the time span on the "Setting up Meridian"
  // day-task card — had nothing to subtract from. Fire-and-forget: a failed
  // stamp costs the duration line, never the wizard.
  useEffect(() => { invoke('mark_setup_started').catch(() => {}) }, [])

  // Step 1 — permissions (live)
  const [perms, setPerms] = useState<Wiz['perms']>({ accessibility: null, screen: null, notifications: null })

  // Step 2 — integrations. The shared <ConnectTrackers> drives the actual
  // connect flows (OAuth + token save); this just holds the live connected-state
  // (get_integrations) so the rail status + completion summary stay accurate.
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)

  // Step 3 — email capture (one-time OTP — see ./signin.tsx). The form owns its
  // own busy/error state; this just holds the result.
  const [signedInEmail, setSignedInEmail] = useState<string | null>(null)

  // Whether the OTP Worker reported "not configured" — discovered reactively
  // from a failed send/verify attempt (see `Wiz.otpNotConfigured`'s doc in
  // steps.tsx), the fresh-clone contributor case with no `OTP_API_URL` set.
  // Starts false and never resets once true.
  const [otpNotConfigured, setOtpNotConfigured] = useState(false)
  const onDevBypass = useCallback(() => setOtpNotConfigured(true), [])

  // Step 4 — intelligence. The provider the whole prose pipeline obeys. Persisted to
  // settings.json immediately on pick (not batched to Finish): if the user quits the
  // wizard halfway, the choice they made should still be the choice that runs.
  const [provider, setProviderState] = useState<LlmProviderId>(DEFAULT_LLM_PROVIDER)
  // Which custom endpoint, when `provider` is 'custom' — see RuntimeSettings.
  const [providerCustomId, setProviderCustomIdState] = useState<string | null>(null)
  const {
    status: providers, scanning: scanningProviders,
    testingIds: testingProviderIds, installingIds: installingProviderIds, signingIds: signingProviderIds,
    testOne: testProvider, install: installProvider, signIn: signInProvider, rescan: rescanProviders,
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

  // Commit the choice, and only THEN move the UI. Settings works this way (it renders
  // `settings.llm_provider` straight off disk); setup used to set local state optimistically
  // and roll back on failure, which is not the same invariant — two overlapping picks could
  // roll back to a value the disk no longer held, and the error it set is cleared by any
  // navigation, so the mismatch could disappear unseen. Returning the promise is what lets
  // the shared detail view render "Switching…" and surface a failure here too.
  const setProvider = useCallback((id: LlmProviderId, customId?: string) => {
    const fields = providerChoiceFields(id, customId) as Partial<RuntimeSettings>
    return mutate<RuntimeSettings>('/api/settings', 'update_settings', fields, 'PUT')
      .then(() => {
        setProviderState(id)
        if (id === 'custom') setProviderCustomIdState(customId ?? null)
      })
      .catch((e) => {
        // Never claim a choice the daemon isn't honouring — the exact silent mismatch
        // update_settings validates against. Rethrown so the detail view can show it.
        setErr(String(e))
        throw e
      })
  }, [])

  const active = !welcome

  // Poll the two required permissions + optional notifications on the
  // Permissions step. Input Monitoring is intentionally not polled — it's
  // redundant with Accessibility (see the note on PERMISSIONS in ./data.ts).
  // Notification state also refreshes here after the user answers the OS
  // dialog or flips the toggle in System Settings.
  useEffect(() => {
    if (!active || steps[step]?.id !== 'permissions') return
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
  }, [active, step, steps])

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
    if (!active || steps[step]?.id !== 'integrations') return
    refetchIntegrations()
    const id = setInterval(refetchIntegrations, 3000)
    return () => clearInterval(id)
  }, [active, step, steps, refetchIntegrations])

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

  // Persists the verified email so the Rust side knows who's signed in even
  // after this webview session ends (see commands::save_account_email).
  // Best-effort: a failed write here never blocks the wizard — `OtpForm`
  // already made its own (also best-effort) call and lets the user through
  // regardless.
  const onSignedIn = useCallback((email: string) => {
    setSignedInEmail(email)
    invoke('save_account_email', { email }).catch(() => {})
  }, [])

  const wiz: Wiz = {
    platform,
    perms, openPane, grantScreen, grantNotifications,
    integrations, refetchIntegrations,
    signedInEmail, onSignedIn, otpNotConfigured, onDevBypass,
    provider, providerCustomId, setProvider, providers, scanningProviders,
    testingProviderIds, installingProviderIds, signingProviderIds,
    testProvider, installProvider, signInProvider, rescanProviders,
  }

  // ── Navigation ───────────────────────────────────────────────────────────────
  const meta = steps[step]
  const last = step === steps.length - 1
  const finish = async () => {
    // `mark_setup_complete` writes the onboarded flag that stops the wizard
    // reopening next launch. Only proceed to the dashboard if it actually
    // persisted — otherwise the user would land there but the wizard would
    // reappear next launch.
    setErr('')
    try {
      await invoke('mark_setup_complete')
    } catch (e) {
      setErr(String(e))
      return
    }
    // FINISHING THE WIZARD IS WHAT ENTITLES THIS ONBOARDING TO THE WALKTHROUGH,
    // so clear the "already seen it" key here.
    //
    // The walkthrough auto-starts on two conditions that live in two different
    // stores with two different lifetimes: `~/.meridian/walkthrough_armed`, which
    // the command above just wrote, and `meridian.walkthrough.seen.v1` in the
    // webview's localStorage. Wiping `~/.meridian` does not touch localStorage —
    // nothing outside the webview can — so a machine that has ever completed the
    // walkthrough kept that key forever.
    //
    // The result: reinstall, or delete ~/.meridian to start over, and setup runs
    // again (correctly) but the walkthrough silently never plays. Nothing fails,
    // nothing logs, the dashboard just opens. That is what happened on the first
    // real test of this flow.
    //
    // Clearing it here makes wizard completion the single authority: you finished
    // onboarding, so this onboarding has not been shown the walkthrough. It also
    // makes Settings → Re-run Setup replay the tour, which is the honest reading
    // of re-running onboarding (and matches `mark_setup_started`, which likewise
    // treats a re-run as a fresh run rather than preserving the original).
    try { localStorage.removeItem('meridian.walkthrough.seen.v1') } catch { /* private mode */ }
    // Onboarding is over, so there is no position left to resume to. Without
    // this, Settings → Re-run Setup would open on the last step of the run
    // that just completed rather than at the beginning.
    try { localStorage.removeItem(PROGRESS_KEY) } catch { /* private mode */ }
    try {
      await invoke('open_dashboard')
    } catch { /* ignore if dashboard fails to open */ }
    tauri()?.window.getCurrentWindow().close()
  }

  // Sign in and Permissions both refuse to be pinned to a magic number. Sign in
  // changes size as you move through it (email phase vs. code phase vs. an
  // error line appearing); Permissions changes with the OS strings and with how
  // its three panes wrap at the current width, and the guessing game there
  // (490 scrolled, 520 left whitespace, 512 as a compromise that scrolled
  // again) is exactly why it is measured now rather than guessed a fourth time.
  // So on these steps the card is `height: auto`, measures its OWN rendered
  // height (`cardRef` + ResizeObserver below), and the window follows it - the
  // content is never the thing that has to fit, so there is nothing to scroll.
  const autoSized = !welcome && (meta?.id === 'email' || meta?.id === 'permissions')
  const isEmailStep = !welcome && meta?.id === 'email'
  const cardRef = useRef<HTMLDivElement>(null)
  const [measuredHeight, setMeasuredHeight] = useState<number | null>(null)
  // Re-runs on every step change so a stale measurement from the previous
  // auto-sized step can't size this one; `null` while unmeasured falls back to
  // the neutral height below for the single frame before the observer fires.
  useLayoutEffect(() => {
    setMeasuredHeight(null)
    if (!autoSized || !cardRef.current) return
    const el = cardRef.current
    const observer = new ResizeObserver(([entry]) => {
      setMeasuredHeight(Math.ceil(entry.contentRect.height))
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [autoSized, meta?.id])

  // The card is a different height depending on what's showing, each sized to
  // that screen's actual content instead of one height shared by everything
  // (which used to leave dead top/bottom whitespace on the shorter screens):
  // Welcome (520, content only runs to ~372px), Sign in and Permissions
  // (measured live, see above), everything else (628, unchanged -
  // Integrations/Intelligence all still need the room).
  const cardHeight = welcome ? 520 : autoSized ? (measuredHeight ?? 520) : 628
  // Height divisor scaled with the card (700 * cardHeight/628) to keep the
  // same ~26px margin ratio the window was sized for at any height.
  const heightDivisor = Math.round((700 * cardHeight) / 628)

  // The host OS window (`tray.rs::open_wizard_window`) opens already sized for
  // Welcome (1000×572) so it doesn't start with a big empty backdrop margin;
  // this keeps it in sync with `cardHeight` from then on - one place that
  // resizes the window, rather than a resize call bolted onto each individual
  // transition (Welcome → step 1, step → step, …). Only re-invokes when the
  // target height actually changes, so paging between same-height steps (or
  // a Sign-in re-measure that comes out the same) is a no-op. Best-effort:
  // outside Tauri (a plain browser preview) or on a slow IPC round-trip, the
  // wizard keeps working rather than getting stuck waiting on the window resize.
  useEffect(() => {
    invoke('resize_setup_window', { width: 1000, height: cardHeight + 52 }).catch(() => {})
  }, [cardHeight])

  // Each of these swaps the whole card's content (Welcome ↔ Rail steps) —
  // exactly the kind of heavy reflow that can trip a WKWebView/wry compositor
  // bug on macOS when the window is already in native full-screen: the
  // webview's drawable gets pinned to a stale size and the rest of the
  // (genuinely full-screen) window renders black. `resize_setup_window` above
  // already guards its own resize against this (`is_fullscreen()` in
  // system.rs), but that only covers transitions where `cardHeight` actually
  // changes AND the window isn't full-screen; this covers every transition,
  // full-screen or not, by forcing a real (same-size) window frame-set that
  // makes WebKit recompute the compositing surface. See nudgeWindowRepaint.
  useEffect(() => { nudgeWindowRepaint() }, [welcome, step])

  return (
    <div style={{
      position: 'fixed', inset: 0, display: 'grid', placeItems: 'center',
      // Subtle top-lit depth so the centred card reads as a distinct surface —
      // otherwise, when the window is enlarged / full-screened, the card floats
      // on a big flat panel.
      background: 'radial-gradient(130% 130% at 50% 0%, color-mix(in srgb, var(--t-card) 34%, var(--t-panel)) 0%, var(--t-panel) 62%)',
    }}>
      <div ref={cardRef} className="rise" style={{
        width: 948, height: autoSized ? 'auto' : cardHeight, borderRadius: 18, background: 'var(--t-card)',
        border: '0.5px solid var(--t-card-border)', overflow: 'hidden', color: 'var(--t-title)',
        boxShadow: 'var(--pop-shadow)',
        // Grow the whole card proportionally as the window grows (macOS
        // full-screen / manual resize) so it stays a prominent, readable surface
        // instead of a small rectangle. clamp floor = 1 (never shrinks below the
        // design size at the default window), cap = 1.7 (won't balloon on a 27").
        // min() of the width- and height-fits guarantees it never exceeds the
        // viewport; vector text scales crisply.
        transform: `scale(clamp(1, min(calc(100vw / 1010), calc(100vh / ${heightDivisor})), 1.7))`,
        transformOrigin: 'center',
      }}>
        {welcome ? (
          <Welcome
            onBegin={() => { setWelcome(false); setStep(0) }}
            // Both inputs to `buildSteps` must have landed before the step list
            // can be navigated - see the `platform` and `notifPrecheck` notes.
            ready={platform !== null && notifPrecheck !== null}
            error={platformError}
            onRetry={resolvePlatform}
          />
        ) : (
          <div className="flex flex-col" style={{ height: '100%' }}>
            <div style={{ padding: '26px 32px 14px', textAlign: isEmailStep ? 'center' : 'left' }}>
              <Kicker style={{ marginBottom: 9 }}>{meta.kicker}</Kicker>
              <h1 style={{ ...DISPLAY, fontSize: 23, lineHeight: 1.1, color: 'var(--t-title)' }}>{meta.title}</h1>
              <p style={{
                fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-muted)', marginTop: 8,
                maxWidth: isEmailStep ? 340 : 620, marginLeft: isEmailStep ? 'auto' : undefined, marginRight: isEmailStep ? 'auto' : undefined,
                textWrap: 'pretty',
              }}>{meta.subtitle}</p>
            </div>
            {/* Scrolls only on the FIXED-height steps. On an auto-sized step the
               card already grew to whatever the body needs, so leaving this
               `auto` would do nothing except let a sub-pixel rounding difference
               between the measured height and the laid-out content produce a
               1px scrollbar over content that fits. */}
            <div className="nice-scroll flex flex-col" style={{ flex: 1, overflowY: autoSized ? 'visible' : 'auto', padding: '4px 32px 22px' }}>
              <meta.Body wiz={wiz} />
            </div>
            <Footer step={step} last={last} canNext={meta.canNext(wiz)} err={err}
              onBack={() => { setErr(''); setStep(Math.max(0, step - 1)) }}
              onNext={() => (last ? finish() : (setErr(''), setStep(step + 1)))} />
          </div>
        )}
      </div>
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
      {/* "Open Dashboard", not "Finish setup". Both are true of the last step,
          but only one of them names what happens when it is pressed - `finish()`
          marks setup complete, opens the dashboard window, and closes this one.
          "Finish setup" described the bookkeeping and left the user to guess
          where they were about to land. */}
      <Btn variant="primary" disabled={!canNext} onClick={onNext}>
        {last ? 'Open Dashboard' : 'Continue'}{!last && <ArrowR />}
      </Btn>
    </div>
  )
}

const ArrowL = (): ReactNode => (<svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="M10 4 6 8l4 4" /></svg>)
const ArrowR = (): ReactNode => (<svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="M6 4l4 4-4 4" /></svg>)
