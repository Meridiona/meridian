//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The onboarding wizard — the first Next route rendered inside a Tauri window
// (the "setup" window opened from tray.rs::open_wizard_window). Talks to Rust
// exclusively over the Tauri `invoke` bridge — no /api fetch, no Node server.

import { useState, useEffect } from 'react'
import { invoke, tauri } from '@/lib/bridge'

const STEPS = ['Welcome', 'Permissions', 'Model', 'Connect', 'Done'] as const

type MlxStatus = 'offline' | 'starting' | 'running' | { error: string }

interface MlxStatusResponse {
  status: MlxStatus
  port: number
  runtime_found: boolean
  runtime_installed: boolean
  download_available: boolean
}

interface DownloadProgress {
  received: number
  total: number
  message: string
}

export default function SetupWizard() {
  const [step, setStep] = useState(0)
  const [err, setErr] = useState('')
  const last = STEPS.length - 1

  // Step 1: live permission state
  const [screenGrant, setScreenGrant] = useState<boolean | null>(null)
  const [a11yGrant, setA11yGrant] = useState<boolean | null>(null)
  const [inputMonGrant, setInputMonGrant] = useState<boolean | null>(null)

  // Step 2: MLX server state + runtime download
  const [mlx, setMlx] = useState<MlxStatusResponse | null>(null)
  const [downloading, setDownloading] = useState(false)
  const [progress, setProgress] = useState<DownloadProgress | null>(null)

  // Poll Screen Recording + Accessibility on step 1
  useEffect(() => {
    if (step !== 1) return
    const poll = async () => {
      const [sr, ax, im] = await Promise.all([
        invoke<boolean>('check_screen_recording').catch(() => false),
        invoke<boolean>('check_accessibility').catch(() => false),
      ])
      setScreenGrant(sr)
      setA11yGrant(ax)
    }
    poll()
        invoke<boolean>('check_input_monitoring').catch(() => false),
    const id = setInterval(poll, 2000)
    return () => clearInterval(id)
  }, [step])
      setInputMonGrant(im)

  // Poll MLX status on step 2; auto-start only when the runtime is provisioned.
  // If no runtime is installed yet, the user downloads it via startDownload().
  useEffect(() => {
    if (step !== 2) return
    const poll = async () => {
      try {
        const s = await invoke<MlxStatusResponse>('get_mlx_status')
        setMlx(s)
        if (s.runtime_found && s.status === 'offline') {
          invoke('start_mlx_server_cmd').catch(() => {})
        }
      } catch (_) {
        // no-op: server not yet available
      }
    }
    poll()
    const id = setInterval(poll, 3000)
    return () => clearInterval(id)
  }, [step])

  // Listen for runtime download progress events while a download is in flight.
  useEffect(() => {
    if (!downloading) return
    let unlisten: (() => void) | undefined
    tauri()
      ?.event.listen<DownloadProgress>('mlx-download-progress', (e) => setProgress(e.payload))
      .then((un) => { unlisten = un })
      .catch(() => {})
    return () => { if (unlisten) unlisten() }
  }, [downloading])

  const startDownload = async () => {
    setErr('')
    setDownloading(true)
    setProgress({ received: 0, total: 0, message: 'Starting…' })
    try {
      await invoke('download_runtime_cmd')
      // Runtime now provisioned — kick the server. The poll will reflect it.
      invoke('start_mlx_server_cmd').catch(() => {})
    } catch (e) {
      setErr(String(e))
    } finally {
      setDownloading(false)
    }
  }

  const openPane = (pane: string) => {
    setErr('')
    invoke('open_permission_pane', { pane }).catch((e) => setErr(String(e)))
  }

  const connectTracker = (provider: string) => {
    setErr('')
    invoke('start_oauth', { provider }).catch((e) => setErr(String(e)))
  }

  const next = async () => {
    setErr('')
    if (step === last) {
      try {
        await invoke('mark_setup_complete')
      } catch (_) {
        // best-effort — don't block the user if the write fails
      }
      tauri()?.window.getCurrentWindow().close()
    } else {
      setStep(step + 1)
    }
  }

  return (
    <div className="flex min-h-screen flex-col">
      {/* Step rail */}
      <div className="flex gap-2 px-6 pt-5">
        {STEPS.map((s, i) => (
          <div
            key={s}
            className={`h-[3px] flex-1 rounded-full transition-colors ${
              i < step ? 'bg-emerald-500' : i === step ? 'bg-blue-500' : 'bg-current/15'
            }`}
          />
        ))}
      </div>

      <main className="flex-1 overflow-y-auto px-8 pt-7 pb-2">
        {step === 0 && (
          <section>
            <h1 className="mb-1.5 text-2xl font-semibold">Welcome to Meridian</h1>
            <p className="mb-3.5 text-[15px] opacity-60">
              Meridian watches what you do and keeps your project tickets up to date — automatically,
              on-device. Let&apos;s get a few things set up. It takes about a minute.
            </p>
            <p className="text-[15px] opacity-60">Nothing leaves your Mac unless you connect a tracker at the end.</p>
          </section>
        )}

        {step === 1 && (
          <section>
            <h1 className="mb-1.5 text-2xl font-semibold">Permissions</h1>
            <p className="mb-3.5 text-[15px] opacity-70">
              Meridian needs three macOS permissions. Open each in System Settings and toggle Meridian on.
            </p>
            <PermissionCard
              title="Screen Recording"
              sub="Reads on-screen text to understand what you're working on."
              granted={screenGrant}
              onOpen={() => openPane('screen_recording')}
            />
            <PermissionCard
              title="Accessibility"
              sub="Reads window titles and UI labels for accurate context."
              granted={a11yGrant}
              onOpen={() => openPane('accessibility')}
            />
            <PermissionCard
              title="Input Monitoring"
              sub="Detects clicks and typing to mark when you switch tasks."
              granted={inputMonGrant}
              onOpen={() => openPane('input_monitoring')}
            />
            {screenGrant && a11yGrant && inputMonGrant && (
              <p className="mt-3 text-[13px] text-emerald-600 font-medium">
                All permissions granted — ready to continue.
              </p>
            )}
          </section>
        )}

        {step === 2 && (
          <section>
            <h1 className="mb-1.5 text-2xl font-semibold">On-device model</h1>
            <p className="mb-3.5 text-[15px] opacity-70">
              Meridian classifies your work with a local model so nothing is sent to the cloud. The
              runtime downloads once, then loads in the background.
            </p>
            <MlxRow
              mlx={mlx}
              downloading={downloading}
              progress={progress}
              onDownload={startDownload}
            />
          </section>
        )}

        {step === 3 && (
          <section>
            <h1 className="mb-1.5 text-2xl font-semibold">Connect a tracker</h1>
            <p className="mb-3.5 text-[15px] opacity-70">
              Connect Jira or Trello so Meridian can update your tickets. Optional — do it later from Settings.
            </p>
            <Row title="Jira" sub="Opens a browser to authorize Meridian.">
              <button
                onClick={() => connectTracker('jira')}
                className="rounded-lg border border-current/20 px-3 py-1.5 text-[13px] hover:bg-current/5 transition-colors"
              >
                Connect
              </button>
            </Row>
            <Row title="Trello" sub="Opens a browser to authorize Meridian.">
              <button
                onClick={() => connectTracker('trello')}
                className="rounded-lg border border-current/20 px-3 py-1.5 text-[13px] hover:bg-current/5 transition-colors"
              >
                Connect
              </button>
            </Row>
            <p className="mt-3 text-[11px] opacity-55">Connecting a tracker is optional — skip with Continue.</p>
          </section>
        )}

        {step === 4 && (
          <section>
            <h1 className="mb-1.5 text-2xl font-semibold">You&apos;re all set</h1>
            <p className="mb-3.5 text-[15px] opacity-60">
              Meridian is running in your menu bar. It&apos;ll start capturing sessions and drafting ticket
              updates as you work.
            </p>
            <p className="text-[15px] opacity-60">Open the dashboard any time from the tray icon.</p>
          </section>
        )}
      </main>

      <footer className="flex items-center justify-between border-t border-current/10 px-8 pb-5 pt-3.5">
        <button
          onClick={() => { setErr(''); setStep(Math.max(0, step - 1)) }}
          className={`rounded-lg border border-current/20 px-4 py-2 text-[14px] ${step === 0 ? 'invisible' : ''}`}
        >
          Back
        </button>
        <span className="text-xs text-red-600">{err}</span>
        <button onClick={next} className="rounded-lg bg-blue-500 px-4 py-2 text-[14px] font-medium text-white hover:bg-blue-600 transition-colors">
          {step === last ? 'Finish' : 'Continue'}
        </button>
      </footer>
    </div>
  )
}

function Row({ title, sub, children }: { title: string; sub: string; children: React.ReactNode }) {
  return (
    <div className="mb-3 flex items-center gap-3.5 rounded-[10px] border border-current/15 px-4 py-3.5">
      <div className="flex-1">
        <div className="font-semibold text-[14px]">{title}</div>
        <div className="text-xs opacity-60 mt-0.5">{sub}</div>
      </div>
      {children}
    </div>
  )
}

function PermissionCard({
  title,
  sub,
  granted,
  onOpen,
}: {
  title: string
  sub: string
  granted: boolean | null
  onOpen: () => void
}) {
  return (
    <Row title={title} sub={sub}>
      {granted === true ? (
        <span className="rounded-full bg-emerald-500/20 px-2.5 py-1 text-xs font-semibold text-emerald-600">
          granted
        </span>
      ) : (
        <>
          <span className="rounded-full bg-orange-500/20 px-2.5 py-1 text-xs font-semibold text-orange-600">
            required
          </span>
          <button
            onClick={onOpen}
            className="rounded-lg border border-current/20 px-3 py-1.5 text-[13px] hover:bg-current/5 transition-colors"
          >
            Open Settings
          </button>
        </>
      )}
    </Row>
  )
}

function MlxRow({
  mlx,
  downloading,
  progress,
  onDownload,
}: {
  mlx: MlxStatusResponse | null
  downloading: boolean
  progress: DownloadProgress | null
  onDownload: () => void
}) {
  // Download in flight — show a progress bar instead of a status badge.
  if (downloading) {
    const pct = progress && progress.total > 0 ? (progress.received / progress.total) * 100 : 0
    return (
      <div className="mb-3 rounded-[10px] border border-current/15 px-4 py-3.5">
        <div className="mb-2 flex items-center justify-between">
          <span className="font-semibold text-[14px]">Downloading runtime</span>
          <span className="text-xs opacity-60">{progress?.message ?? 'Starting…'}</span>
        </div>
        <div className="h-2 overflow-hidden rounded-full bg-current/10">
          <div
            className="h-full rounded-full bg-blue-500 transition-all"
            style={{ width: progress && progress.total > 0 ? `${pct}%` : '100%' }}
          />
        </div>
      </div>
    )
  }

  let badge: React.ReactNode
  let sub: string

  if (!mlx) {
    badge = <span className="rounded-full bg-current/10 px-2.5 py-1 text-xs opacity-50">checking…</span>
    sub = 'Checking server status…'
  } else if (mlx.status === 'running') {
    badge = <span className="rounded-full bg-emerald-500/20 px-2.5 py-1 text-xs font-semibold text-emerald-600">running</span>
    sub = `Classifier ready on port ${mlx.port}.`
  } else if (mlx.status === 'starting') {
    badge = <span className="rounded-full bg-blue-500/20 px-2.5 py-1 text-xs font-semibold text-blue-600">starting…</span>
    sub = 'Server is loading the model — this may take a moment on first run.'
  } else if (mlx.runtime_found) {
    // Runtime present (downloaded or dev), server just not up yet.
    badge = <span className="rounded-full bg-current/10 px-2.5 py-1 text-xs opacity-50">offline</span>
    sub = 'Runtime installed — attempting to start the server…'
  } else if (mlx.download_available) {
    // No runtime, but a download is configured — offer the download button.
    return (
      <Row
        title="Classifier model (Qwen3)"
        sub="Download the on-device runtime (~750 MB) and model. This happens once."
      >
        <button
          onClick={onDownload}
          className="rounded-lg bg-blue-500 px-3 py-1.5 text-[13px] font-medium text-white hover:bg-blue-600 transition-colors"
        >
          Download
        </button>
      </Row>
    )
  } else {
    // No runtime and no download URL configured yet.
    badge = <span className="rounded-full bg-orange-500/20 px-2.5 py-1 text-xs font-semibold text-orange-600">not available</span>
    sub = 'The downloadable runtime is not published yet. For dev, run: cd services && uv sync.'
  }

  return <Row title="Classifier model (Qwen3)" sub={sub}>{badge}</Row>
}
