//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The onboarding wizard — the first Next route rendered inside a Tauri window
// (the "setup" window, see tray/src-tauri/src/lib.rs::open_wizard_window). It
// talks to Rust exclusively over the Tauri `invoke` bridge — no /api fetch, no
// Node server. This is the template every folded-in native window follows.

import { useState } from 'react'
import { invoke, tauri } from '@/lib/bridge'

const STEPS = ['Welcome', 'Permissions', 'Model', 'Connect', 'Done'] as const

export default function SetupWizard() {
  const [step, setStep] = useState(0)
  const [err, setErr] = useState('')
  const last = STEPS.length - 1

  const openPane = (pane: string) => {
    setErr('')
    invoke('open_permission_pane', { pane }).catch((e) => setErr(String(e)))
  }

  const next = () => {
    setErr('')
    if (step < last) setStep(step + 1)
    else tauri()?.window.getCurrentWindow().close() // Finish — close the wizard window.
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
            <p className="mb-3.5">
              Meridian needs two macOS permissions to see your activity. Open each in System Settings and
              toggle Meridian on.
            </p>
            <PermissionCard
              title="Screen Recording"
              sub="Reads on-screen text to understand what you're working on."
              onOpen={() => openPane('screen_recording')}
            />
            <PermissionCard
              title="Accessibility"
              sub="Reads window titles and UI labels for accurate context."
              onOpen={() => openPane('accessibility')}
            />
            <p className="mt-3 text-[11px] opacity-55">
              Live grant detection (confirmed by data actually flowing) is wired in the next slice.
            </p>
          </section>
        )}

        {step === 2 && (
          <section>
            <h1 className="mb-1.5 text-2xl font-semibold">On-device model</h1>
            <p className="mb-3.5">
              Meridian classifies your work with a local model so nothing is sent to the cloud. It loads in
              the background — you can keep going.
            </p>
            <Row title="Classifier model" sub="Loads automatically in the background. Live status is wired in a later slice.">
              <span className="rounded-full bg-current/10 px-2.5 py-1 text-xs font-semibold opacity-60">background</span>
            </Row>
          </section>
        )}

        {step === 3 && (
          <section>
            <h1 className="mb-1.5 text-2xl font-semibold">Connect a tracker</h1>
            <p className="mb-3.5">
              Connect Jira or Trello so Meridian can update your tickets. Optional — you can do it later from
              the dashboard.
            </p>
            <Row title="Jira" sub="Opens a browser to authorize Meridian.">
              <button disabled className="rounded-lg border border-current/20 px-3 py-1.5 text-[13px] opacity-45">
                Connect
              </button>
            </Row>
            <Row title="Trello" sub="Opens a browser to authorize Meridian.">
              <button disabled className="rounded-lg border border-current/20 px-3 py-1.5 text-[13px] opacity-45">
                Connect
              </button>
            </Row>
            <p className="mt-3 text-[11px] opacity-55">Tracker auth is wired in the next step.</p>
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
          onClick={() => setStep(Math.max(0, step - 1))}
          className={`rounded-lg border border-current/20 px-4 py-2 ${step === 0 ? 'invisible' : ''}`}
        >
          Back
        </button>
        <span className="text-xs text-red-600">{err}</span>
        <button onClick={next} className="rounded-lg bg-blue-500 px-4 py-2 font-medium text-white">
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
        <div className="font-semibold">{title}</div>
        <div className="text-xs opacity-60">{sub}</div>
      </div>
      {children}
    </div>
  )
}

function PermissionCard({ title, sub, onOpen }: { title: string; sub: string; onOpen: () => void }) {
  return (
    <Row title={title} sub={sub}>
      <span className="rounded-full bg-orange-500/20 px-2.5 py-1 text-xs font-semibold text-orange-600">required</span>
      <button onClick={onOpen} className="rounded-lg border border-current/20 px-3 py-1.5 text-[13px]">
        Open Settings
      </button>
    </Row>
  )
}
