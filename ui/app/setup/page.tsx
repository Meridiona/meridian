//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The first-run onboarding wizard — the "A · Rail" shell from the Meridian Setup
// design, wired to the real backend. Renders inside the Tauri "setup" window
// (tray.rs::open_wizard_window) and talks to Rust exclusively over the `invoke`
// bridge. Presentation comes from the design (atoms/steps/data); behaviour —
// permission polling, MLX status + download, hardware specs, model selection,
// OAuth — is all live. No fabricated state.

import { useState, useEffect, useRef, useCallback } from 'react'
import type { CSSProperties, ReactNode } from 'react'
import { invoke, load, tauri } from '@/lib/bridge'
import { STEPS, Welcome, Completion } from './steps'
import type { Wiz } from './steps'
import { MODELS, MODEL_BY_ID } from './data'
import type { DownloadProgress, MlxStatusResponse, ModelTier, SystemSpecs } from './data'
import type { IntegrationsResponse } from '@/lib/api-types'
import { Btn, Check, Kicker } from './atoms'

const SERIF: CSSProperties = { fontFamily: 'var(--font-instrument-serif), Georgia, serif' }

export default function SetupWizard() {
  const [welcome, setWelcome] = useState(true)
  const [step, setStep] = useState(0)
  const [done, setDone] = useState(false)
  const [err, setErr] = useState('')

  // Step 1 — permissions (live)
  const [perms, setPerms] = useState<Wiz['perms']>({ accessibility: null, screen: null, input: null })

  // Step 2 — specs + MLX + model
  const [specs, setSpecs] = useState<SystemSpecs | null>(null)
  const [mlx, setMlx] = useState<MlxStatusResponse | null>(null)
  const [downloading, setDownloading] = useState(false)
  const [prefetching, setPrefetching] = useState(false)
  const [modelReady, setModelReady] = useState(false)
  const [progress, setProgress] = useState<DownloadProgress | null>(null)
  const [model, setModel] = useState<ModelTier['id']>('core')
  // The user must explicitly commit a model (the Download button) before we
  // prefetch — otherwise an auto-prefetch would fetch the *default* before they
  // pick, then a later pick would mislabel the row as "Ready" for a model the
  // server never loaded. `wantModel` is that commit gate.
  const [wantModel, setWantModel] = useState(false)
  const prefetchStarted = useRef(false)

  // Step 3 — integrations. The shared <ConnectTrackers> drives the actual
  // connect flows (OAuth + token save); this just holds the live connected-state
  // (get_integrations) so the rail status + completion summary stay accurate.
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)

  const active = !welcome && !done

  // Detect hardware once on mount + restore any persisted model choice. Both are
  // cheap one-shots; specs are ready by the time the user reaches the Model step.
  useEffect(() => {
    invoke<SystemSpecs>('detect_system_specs').then(setSpecs).catch(() => {})
    invoke<string | null>('get_model_preference')
      .then((id) => { const t = MODELS.find((m) => m.hfId === id); if (t) setModel(t.id) })
      .catch(() => {})
  }, [])

  // Poll the three required permissions on the Permissions step.
  useEffect(() => {
    if (!active || step !== 0) return
    const poll = async () => {
      const [accessibility, screen, input] = await Promise.all([
        invoke<boolean>('check_accessibility').catch(() => false),
        invoke<boolean>('check_screen_recording').catch(() => false),
        invoke<boolean>('check_input_monitoring').catch(() => false),
      ])
      setPerms({ accessibility, screen, input })
    }
    poll()
    const id = setInterval(poll, 2000)
    return () => clearInterval(id)
  }, [active, step])

  // Poll MLX status while on the Model step OR while a commit is in flight (so a
  // model that's still downloading keeps progressing after the user clicks
  // Continue). Auto-starts the server when the runtime is present; only prefetches
  // once the user has committed a model (`wantModel`), guaranteeing the chosen
  // preference is on disk before the server resolves which model to fetch.
  useEffect(() => {
    const polling = active && !modelReady && (step === 1 || wantModel || downloading || prefetching)
    if (!polling) return
    const poll = async () => {
      try {
        const s = await invoke<MlxStatusResponse>('get_mlx_status')
        setMlx(s)
        if (s.runtime_found && s.status === 'offline') invoke('start_mlx_server_cmd').catch(() => {})
        if (wantModel && s.status === 'running' && !prefetchStarted.current) {
          prefetchStarted.current = true
          setPrefetching(true)
          setProgress({ received: 0, total: 0, message: 'Preparing model…' })
          invoke('prefetch_model_cmd')
            .then(() => setModelReady(true))
            .catch((e) => {
              setErr(String(e))
              // Re-arm so the Download button can retry: clear the one-shot guard
              // and the commit flag, dropping the row back to an actionable state.
              prefetchStarted.current = false
              setWantModel(false)
            })
            .finally(() => setPrefetching(false))
        }
      } catch { /* server not yet available */ }
    }
    poll()
    const id = setInterval(poll, 3000)
    return () => clearInterval(id)
  }, [active, step, modelReady, wantModel, downloading, prefetching])

  // Stream download progress (shared by the runtime download + the model prefetch).
  useEffect(() => {
    if (!downloading && !prefetching) return
    let unlisten: (() => void) | undefined
    // Guard against the listener resolving AFTER this effect has been cleaned up
    // (downloading/prefetching flipped false): without `cancelled`, the late
    // `unlisten` would never run and a stale listener would double-fire progress.
    let cancelled = false
    tauri()?.event.listen<DownloadProgress>('mlx-download-progress', (e) => setProgress(e.payload))
      .then((un) => { if (cancelled) un(); else unlisten = un }).catch(() => {})
    return () => { cancelled = true; if (unlisten) unlisten() }
  }, [downloading, prefetching])

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
    if (!active || step !== 2) return
    refetchIntegrations()
    const id = setInterval(refetchIntegrations, 3000)
    return () => clearInterval(id)
  }, [active, step, refetchIntegrations])

  // ── Actions ────────────────────────────────────────────────────────────────
  const openPane = useCallback((pane: string) => {
    setErr(''); invoke('open_permission_pane', { pane }).catch((e) => setErr(String(e)))
  }, [])

  // Screen Recording needs an explicit request to register the app before the
  // Settings pane shows anything to toggle (same pattern as grantInput).
  const grantScreen = useCallback(async () => {
    setErr('')
    try { await invoke('request_screen_recording') } catch { /* prompt is best-effort */ }
    invoke('open_permission_pane', { pane: 'screen_recording' }).catch((e) => setErr(String(e)))
  }, [])

  // Input Monitoring needs an explicit request to register the app before the
  // Settings pane shows anything to toggle (mirrors the original wizard).
  const grantInput = useCallback(async () => {
    setErr('')
    try { await invoke('request_input_monitoring') } catch { /* prompt is best-effort */ }
    invoke('open_permission_pane', { pane: 'input_monitoring' }).catch((e) => setErr(String(e)))
  }, [])

  const selectModel = useCallback((id: ModelTier['id']) => {
    setModel(id)
    invoke('set_model_preference', { modelId: MODEL_BY_ID[id].hfId }).catch(() => {})
  }, [])

  // Provision the MLX runtime tarball, then bring the server up. No model is
  // fetched here — that waits for an explicit Download (downloadModel).
  const installRuntime = useCallback(async () => {
    setErr('')
    setDownloading(true)
    setProgress({ received: 0, total: 0, message: 'Starting…' })
    try {
      await invoke('download_runtime_cmd')
      invoke('start_mlx_server_cmd').catch(() => {})
    } catch (e) { setErr(String(e)) } finally { setDownloading(false) }
  }, [])

  // Commit the chosen model: persist the preference FIRST, then flag `wantModel`
  // so the poll prefetches it once the server is running. Writing before the
  // prefetch is what makes the "Ready" badge truthful about which model loaded.
  const downloadModel = useCallback(async () => {
    setErr('')
    // The preference MUST persist before the server prefetches — otherwise a
    // failed write would let the server download the old/default model while the
    // UI marks the newly-selected one ready. Stop the commit if the write fails.
    try {
      await invoke('set_model_preference', { modelId: MODEL_BY_ID[model].hfId })
    } catch (e) {
      setErr(String(e))
      setWantModel(false)
      prefetchStarted.current = false
      return
    }
    invoke('start_mlx_server_cmd').catch(() => {})
    setWantModel(true)
    setProgress({ received: 0, total: 0, message: 'Preparing model…' })
  }, [model])

  const wiz: Wiz = {
    perms, openPane, grantScreen, grantInput,
    specs, mlx, model, selectModel, downloading, prefetching, modelReady, progress,
    committing: wantModel && !prefetching && !modelReady,
    installRuntime, downloadModel,
    integrations, refetchIntegrations,
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
    <div style={{ position: 'fixed', inset: 0, display: 'grid', placeItems: 'center', background: 'var(--paper)' }}>
      <div className="rise" style={{
        width: 948, height: 628, borderRadius: 18, background: 'var(--surface)',
        border: '0.5px solid var(--rule-2)', overflow: 'hidden', color: 'var(--ink)',
        boxShadow: 'var(--pop-shadow)',
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
                    <h1 style={{ ...SERIF, fontSize: 27, lineHeight: 1.04, letterSpacing: '-.01em', color: 'var(--ink)' }}>{meta.title}</h1>
                    <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--ink-2)', marginTop: 8, maxWidth: 460, textWrap: 'pretty' }}>{meta.subtitle}</p>
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
    <div className="flex flex-col" style={{ width: 250, flexShrink: 0, background: 'var(--surface-2)', borderRight: '1px solid var(--rule)', padding: '22px 18px' }}>
      <div style={{ padding: '0 6px', marginBottom: 26 }}>
        <div className="flex items-center" style={{ gap: 8 }}>
          <span style={{ width: 8, height: 8, borderRadius: 99, background: 'var(--accent)' }} />
          <span style={{ ...SERIF, fontSize: 21, lineHeight: 1, letterSpacing: '.01em', color: 'var(--ink)' }}>meridian</span>
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
                background: isCur ? 'var(--tint)' : 'transparent', transition: 'background .14s' }}
              onMouseEnter={(e) => { if (!isCur && reachable) e.currentTarget.style.background = 'var(--surface)' }}
              onMouseLeave={(e) => { if (!isCur) e.currentTarget.style.background = 'transparent' }}>
              <span className="flex items-center justify-center font-mono shrink-0" style={{
                width: 24, height: 24, borderRadius: 99, fontSize: 11, fontWeight: 600, marginTop: 1,
                background: ok ? 'var(--accent)' : isCur ? 'var(--surface)' : 'transparent',
                color: ok ? '#fff' : isCur ? 'var(--accent)' : 'var(--ink-4)',
                border: ok ? 'none' : `1px solid ${isCur ? 'var(--accent)' : 'var(--rule-2)'}`,
              }}>{ok ? <Check size={13} color="#fff" /> : s.n}</span>
              <div style={{ minWidth: 0, paddingTop: 1 }}>
                <p style={{ fontSize: 13, fontWeight: isCur ? 500 : 400, color: reached ? 'var(--ink)' : 'var(--ink-3)' }}>{s.label}</p>
                <p className="font-mono" style={{ fontSize: 10, color: ok ? 'var(--success)' : 'var(--ink-4)', marginTop: 2, letterSpacing: '.02em' }}>{s.status(wiz)}</p>
              </div>
            </button>
          )
        })}
      </div>
      <div style={{ flex: 1 }} />
      <p className="font-mono" style={{ fontSize: 10, letterSpacing: '.12em', color: 'var(--ink-4)', padding: '0 8px', textTransform: 'uppercase' }}>First-run setup</p>
    </div>
  )
}

// ── Footer ────────────────────────────────────────────────────────────────────
function Footer({ step, last, canNext, err, onBack, onNext }: {
  step: number; last: boolean; canNext: boolean; err: string; onBack: () => void; onNext: () => void
}) {
  return (
    <div className="flex items-center justify-between" style={{ padding: '16px 28px', borderTop: '1px solid var(--rule)', background: 'var(--surface-2)' }}>
      <Btn variant="ghost" disabled={step === 0} onClick={onBack}><ArrowL />Back</Btn>
      <span style={{ fontSize: 11, color: 'var(--warn)', flex: 1, textAlign: 'center', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', padding: '0 12px' }}>{err}</span>
      <Btn variant="primary" disabled={!canNext} onClick={onNext}>
        {last ? 'Finish setup' : 'Continue'}{!last && <ArrowR />}
      </Btn>
    </div>
  )
}

const ArrowL = (): ReactNode => (<svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="M10 4 6 8l4 4" /></svg>)
const ArrowR = (): ReactNode => (<svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="M6 4l4 4-4 4" /></svg>)
