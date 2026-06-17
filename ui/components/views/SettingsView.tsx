//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { Select } from '@/components/ui/Select'
import { Switch } from '@/components/ui/Switch'
import { NumberStepper } from '@/components/ui/NumberStepper'
import { TextInput } from '@/components/ui/TextInput'
import type { RuntimeSettings } from '@/lib/settings'
import { load } from '@/lib/bridge'

type SaveStatus = 'idle' | 'saved' | 'error'
type ReloadStatus = 'idle' | 'saving' | 'installing' | 'reloading' | 'done' | 'error'

// Poll GET /api/openobserve until OpenObserve is reachable or the background
// install fails. Returns true when reachable. Up to ~90 s (binary download).
async function pollOpenObserveReady(): Promise<boolean> {
  for (let i = 0; i < 60; i++) {
    try {
      const r = await fetch('/api/openobserve')
      const s = await r.json() as { reachable?: boolean; failed?: boolean }
      if (s.reachable) return true
      if (s.failed) return false
    } catch { /* keep polling */ }
    await new Promise(res => setTimeout(res, 1500))
  }
  return false
}

function SectionCard({ children }: { children: React.ReactNode }) {
  return (
    <div style={{
      background: 'var(--surface)',
      border: '1px solid var(--rule)',
      borderRadius: '10px',
      padding: '20px',
      display: 'flex',
      flexDirection: 'column',
      gap: '16px',
    }}>
      {children}
    </div>
  )
}

function SectionHeader({ children }: { children: React.ReactNode }) {
  return (
    <p style={{ fontSize: '10px', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.15em', color: 'var(--ink-3)' }}>
      {children}
    </p>
  )
}

function FieldRow({ label, description, children }: { label: string; description?: string; children: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '24px' }}>
      <div style={{ minWidth: 0 }}>
        <p style={{ fontSize: '13px', fontWeight: 500, color: 'var(--ink)' }}>{label}</p>
        {description && <p style={{ fontSize: '11px', color: 'var(--ink-3)', marginTop: '2px' }}>{description}</p>}
      </div>
      <div style={{ flexShrink: 0, display: 'flex', alignItems: 'center', gap: '8px' }}>{children}</div>
    </div>
  )
}

function SaveButton({ onClick, status }: { onClick: () => void; status: SaveStatus }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: '10px', paddingTop: '8px', borderTop: '1px solid var(--rule)' }}>
      <button
        type="button"
        onClick={onClick}
        style={{
          background: 'var(--accent)',
          color: '#fff',
          fontSize: '12px',
          fontWeight: 500,
          padding: '5px 14px',
          borderRadius: '6px',
          border: 'none',
          boxShadow: '0 1px 3px rgba(0,0,0,0.15)',
        }}
      >
        Save
      </button>
      {status === 'saved' && <span style={{ fontSize: '12px', color: 'var(--success)' }}>Saved</span>}
      {status === 'error' && <span style={{ fontSize: '12px', color: 'var(--warn)' }}>Failed to save</span>}
    </div>
  )
}

const LOG_LEVEL_OPTIONS = [
  { value: 'DEBUG',   label: 'DEBUG' },
  { value: 'INFO',    label: 'INFO' },
  { value: 'WARNING', label: 'WARNING' },
  { value: 'ERROR',   label: 'ERROR' },
]

export default function SettingsView() {
  const [settings, setSettings] = useState<RuntimeSettings | null>(null)
  const [reloadStatus, setReloadStatus] = useState<ReloadStatus>('idle')
  const [reloadMsg, setReloadMsg] = useState<string | null>(null)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const [etlStatus, setEtlStatus] = useState<SaveStatus>('idle')
  const [classificationStatus, setClassificationStatus] = useState<SaveStatus>('idle')
  const [llmStatus, setLlmStatus] = useState<SaveStatus>('idle')
  const [jiraStatus, setJiraStatus] = useState<SaveStatus>('idle')
  const [notifStatus, setNotifStatus] = useState<SaveStatus>('idle')

  useEffect(() => {
    // get_settings (Rust) in the Tauri window, /api/settings in a browser.
    // The PUT saves (below) stay on fetch until the write route is ported.
    load<RuntimeSettings>('/api/settings', 'get_settings')
      .then(setSettings)
      .catch(() => {})
  }, [])

  function patch(changes: Partial<RuntimeSettings>) {
    if (!settings) return
    setSettings({ ...settings, ...changes })
  }

  async function save(fields: Partial<RuntimeSettings>, setStatus: (s: SaveStatus) => void) {
    setStatus('idle')
    try {
      const res = await fetch('/api/settings', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(fields),
      })
      if (!res.ok) throw new Error('non-ok')
      const updated: RuntimeSettings = await res.json()
      setSettings(updated)
      setStatus('saved')
      setTimeout(() => setStatus('idle'), 2000)
    } catch {
      setStatus('error')
      setTimeout(() => setStatus('idle'), 3000)
    }
  }

  // Save observability settings, start/stop the OpenObserve service to match
  // the toggle, then send SIGHUP so the daemon restarts and picks up the new
  // OTLP config. Log-level changes are hot-reloaded in-process (no restart
  // needed); this button handles the credential/toggle/endpoint case.
  const applyObservability = useCallback(async () => {
    if (!settings) return
    setReloadMsg(null)
    setReloadStatus('saving')
    try {
      const res = await fetch('/api/settings', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          log_level: settings.log_level,
          otlp_enabled: settings.otlp_enabled,
          otlp_endpoint: settings.otlp_endpoint,
          oo_email: settings.oo_email,
          oo_password: settings.oo_password,
        }),
      })
      if (!res.ok) throw new Error('save failed')
      const updated: RuntimeSettings = await res.json()
      setSettings(updated)
    } catch {
      setReloadStatus('error')
      setTimeout(() => setReloadStatus('idle'), 3000)
      return
    }

    // The toggle gates the OpenObserve SERVICE itself, not just the exporters:
    // enabled → start the launchd agent (installing it first on a fresh
    // machine); disabled → stop it (and keep it off across logins). A failed
    // start is a real error the user must see — otherwise "Apply" reports
    // success while OpenObserve is down.
    try {
      const ooRes = await fetch('/api/openobserve', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ enabled: settings.otlp_enabled }),
      })
      if (!ooRes.ok) {
        const b = await ooRes.json().catch(() => ({})) as { error?: string }
        setReloadMsg(b.error ?? 'OpenObserve start failed')
        setReloadStatus('error')
        setTimeout(() => { setReloadStatus('idle'); setReloadMsg(null) }, 8000)
        return
      }
      // Fresh machine: the server is downloading + installing OpenObserve in
      // the background. Poll until it is reachable (binary download can take
      // ~30 s) before continuing to the daemon reload.
      const ooBody = await ooRes.json() as { installing?: boolean }
      if (ooBody.installing) {
        setReloadStatus('installing')
        const ready = await pollOpenObserveReady()
        if (!ready) {
          setReloadStatus('error')
          setTimeout(() => setReloadStatus('idle'), 4000)
          return
        }
      }
    } catch {
      setReloadStatus('error')
      setTimeout(() => setReloadStatus('idle'), 3000)
      return
    }

    setReloadStatus('reloading')
    const reloadRes = await fetch('/api/daemon/reload', { method: 'POST' })
    if (reloadRes.status === 503) {
      // Daemon not running (e.g. dev session with the stack down) — settings
      // are saved and will be read at the next daemon start. Not an error.
      setReloadStatus('done')
      setTimeout(() => setReloadStatus('idle'), 3000)
      return
    }
    if (!reloadRes.ok) {
      setReloadStatus('error')
      setTimeout(() => setReloadStatus('idle'), 3000)
      return
    }

    // Poll daemon/status until it responds again (daemon restarted).
    // Give the daemon up to 15 s to come back up.
    let attempts = 0
    pollRef.current = setInterval(async () => {
      attempts++
      try {
        const s = await fetch('/api/daemon/status')
        const { running } = await s.json() as { running: boolean }
        if (running) {
          clearInterval(pollRef.current!)
          pollRef.current = null
          setReloadStatus('done')
          setTimeout(() => setReloadStatus('idle'), 3000)
        }
      } catch { /* keep polling */ }
      if (attempts >= 30) {
        clearInterval(pollRef.current!)
        pollRef.current = null
        setReloadStatus('error')
        setTimeout(() => setReloadStatus('idle'), 3000)
      }
    }, 500)
  }, [settings])

  if (!settings) {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
        {[1, 2, 3].map(i => (
          <div key={i} style={{ height: '120px', borderRadius: '10px', background: 'var(--surface)', opacity: 0.6 }} />
        ))}
      </div>
    )
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '24px' }}>
      <div>
        <h1 style={{ fontSize: '22px', fontWeight: 600, letterSpacing: '-0.02em', color: 'var(--ink)' }}>Settings</h1>
        <p style={{ fontSize: '12px', color: 'var(--ink-3)', marginTop: '4px' }}>Runtime configuration — changes take effect on the next daemon tick.</p>
      </div>

      {/* Observability */}
      <SectionCard>
        <SectionHeader>Observability</SectionHeader>
        {/* Master switch first: when off, OpenObserve is disabled by default and
            the connection fields stay hidden — they appear only once enabled. */}
        <FieldRow label="OpenObserve Export" description="Send traces and logs to the local OpenObserve instance. Off by default; enabling reveals the connection fields. Apply starts/stops OpenObserve and restarts the daemon for you.">
          <Switch checked={settings.otlp_enabled} onCheckedChange={v => patch({ otlp_enabled: v })} />
        </FieldRow>
        <FieldRow label="Log Level" description="Verbosity of daemon logs — always applies to the local log files, and to OpenObserve export when enabled. DEBUG logs everything; WARNING/ERROR suppress info. Hot-reloads on the next daemon tick.">
          <Select
            value={settings.log_level}
            onValueChange={v => patch({ log_level: v as RuntimeSettings['log_level'] })}
            options={LOG_LEVEL_OPTIONS}
          />
        </FieldRow>
        {settings.otlp_enabled && (
          <>
            <FieldRow label="Email" description="Your OpenObserve login. First time? Just pick an email and password here — they become the OpenObserve root account when the service first starts. Already using OpenObserve? Enter the credentials you log in with.">
              <TextInput
                type="email"
                value={settings.oo_email}
                onChange={v => patch({ oo_email: v })}
                placeholder="you@example.com"
              />
            </FieldRow>
            <FieldRow label="Password" description="Stored locally; used to log in at localhost:5080 and as auth for trace/log export.">
              <TextInput
                type="password"
                value={settings.oo_password}
                onChange={v => patch({ oo_password: v })}
                placeholder="••••••••"
              />
            </FieldRow>
            <FieldRow label="OTLP Endpoint (optional)" description="Advanced — leave blank for the local OpenObserve instance. Only set this to export to a remote collector.">
              <TextInput
                value={settings.otlp_endpoint}
                onChange={v => patch({ otlp_endpoint: v })}
                placeholder="http://localhost:5080/api/default/v1/traces"
              />
            </FieldRow>
          </>
        )}
        <div style={{ display: 'flex', alignItems: 'center', gap: '10px', paddingTop: '8px', borderTop: '1px solid var(--rule)' }}>
          {/* Log Level → hot-reloaded in-process (no restart). All other OTel
              fields require the daemon to rebuild its exporters — "Apply" saves
              settings then sends SIGHUP to ONLY the daemon PID; launchd's
              KeepAlive relaunches that single service, leaving screenpipe / MLX /
              UI untouched. */}
          <button
            type="button"
            onClick={applyObservability}
            disabled={reloadStatus === 'saving' || reloadStatus === 'installing' || reloadStatus === 'reloading'}
            style={{
              background: 'var(--accent)',
              color: '#fff',
              fontSize: '12px',
              fontWeight: 500,
              padding: '5px 14px',
              borderRadius: '6px',
              border: 'none',
              cursor: reloadStatus === 'saving' || reloadStatus === 'installing' || reloadStatus === 'reloading' ? 'not-allowed' : 'default',
              opacity: reloadStatus === 'saving' || reloadStatus === 'installing' || reloadStatus === 'reloading' ? 0.7 : 1,
              boxShadow: '0 1px 3px rgba(0,0,0,0.15)',
            }}
          >
            {reloadStatus === 'saving' ? 'Saving…'
              : reloadStatus === 'installing' ? 'Installing…'
              : reloadStatus === 'reloading' ? 'Reloading…'
              : 'Apply'}
          </button>
          {reloadStatus === 'done' && <span style={{ fontSize: '12px', color: 'var(--success)' }}>Active</span>}
          {reloadStatus === 'error' && <span style={{ fontSize: '12px', color: 'var(--warn)' }}>{reloadMsg ?? 'Failed'}</span>}
          <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>
            {reloadStatus === 'installing' ? 'Downloading & installing OpenObserve (first time only)…'
              : reloadStatus === 'reloading' ? 'Restarting daemon…'
              : 'Apply handles everything — installs/starts/stops OpenObserve and restarts the daemon'}
          </span>
          {settings.otlp_enabled && (
            <button
              type="button"
              onClick={() => {
                let base = 'http://localhost:5080'
                try {
                  if (settings.otlp_endpoint) base = new URL(settings.otlp_endpoint).origin
                } catch { /* keep default */ }
                window.open(base, '_blank', 'noopener,noreferrer')
              }}
              style={{
                background: 'transparent',
                color: 'var(--accent)',
                fontSize: '12px',
                fontWeight: 500,
                padding: '5px 14px',
                borderRadius: '6px',
                border: '1px solid var(--accent)',
                cursor: 'default',
                marginLeft: 'auto',
              }}
            >
              Open OpenObserve
            </button>
          )}
        </div>
      </SectionCard>

      {/* ETL Pipeline */}
      <SectionCard>
        <SectionHeader>ETL Pipeline</SectionHeader>
        <FieldRow label="Poll Interval" description="How often the ETL pipeline runs. Takes effect on the next tick.">
          <NumberStepper value={settings.poll_interval_secs} onChange={v => patch({ poll_interval_secs: v })} min={10} max={3600} step={10} />
          <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>sec</span>
        </FieldRow>
        <SaveButton status={etlStatus} onClick={() => save({ poll_interval_secs: settings.poll_interval_secs }, setEtlStatus)} />
      </SectionCard>

      {/* Session Classification */}
      <SectionCard>
        <SectionHeader>Session Classification</SectionHeader>
        <FieldRow label="Classification Enabled">
          <Switch checked={settings.classification_enabled} onCheckedChange={v => patch({ classification_enabled: v })} />
        </FieldRow>
        <FieldRow label="Min Session Duration" description="Sessions shorter than this are skipped by the classifier.">
          <NumberStepper value={settings.min_classification_duration_s} onChange={v => patch({ min_classification_duration_s: v })} min={1} />
          <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>sec</span>
        </FieldRow>
        <FieldRow label="Classification Timeout" description="Maximum time allowed per classification request.">
          <NumberStepper value={settings.classification_timeout_s} onChange={v => patch({ classification_timeout_s: v })} min={5} max={600} step={5} />
          <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>sec</span>
        </FieldRow>
        <FieldRow label="Auto-route Floor" description="Confidence above this → auto-link to task.">
          <NumberStepper value={settings.agent_auto_floor} onChange={v => patch({ agent_auto_floor: v })} min={0} max={1} step={0.05} />
        </FieldRow>
        <FieldRow label="Queue Floor" description="Confidence above this → queue for review.">
          <NumberStepper value={settings.agent_queue_floor} onChange={v => patch({ agent_queue_floor: v })} min={0} max={1} step={0.05} />
        </FieldRow>
        <SaveButton
          status={classificationStatus}
          onClick={() => save({
            classification_enabled: settings.classification_enabled,
            min_classification_duration_s: settings.min_classification_duration_s,
            classification_timeout_s: settings.classification_timeout_s,
            agent_auto_floor: settings.agent_auto_floor,
            agent_queue_floor: settings.agent_queue_floor,
          }, setClassificationStatus)}
        />
      </SectionCard>

      {/* LLM */}
      <SectionCard>
        <SectionHeader>LLM</SectionHeader>
        <FieldRow label="Prefer Local Model" description="Use Apple Silicon MLX or local LM Studio when available.">
          <Switch checked={settings.llm_prefer_local} onCheckedChange={v => patch({ llm_prefer_local: v })} />
        </FieldRow>
        <FieldRow label="Local Budget" description="Fraction of GPU headroom to allow.">
          <NumberStepper value={settings.llm_budget_pct} onChange={v => patch({ llm_budget_pct: v })} min={0} max={1} step={0.05} />
        </FieldRow>
        <SaveButton
          status={llmStatus}
          onClick={() => save({ llm_prefer_local: settings.llm_prefer_local, llm_budget_pct: settings.llm_budget_pct }, setLlmStatus)}
        />
      </SectionCard>

      {/* Jira Updater */}
      <SectionCard>
        <SectionHeader>Jira Updater</SectionHeader>
        <FieldRow label="Jira Updates Enabled">
          <Switch checked={settings.jira_update_enabled} onCheckedChange={v => patch({ jira_update_enabled: v })} />
        </FieldRow>
        <SaveButton status={jiraStatus} onClick={() => save({ jira_update_enabled: settings.jira_update_enabled }, setJiraStatus)} />
      </SectionCard>

      {/* Notifications */}
      <SectionCard>
        <SectionHeader>Notifications</SectionHeader>
        <FieldRow label="Notifications" description="Master switch for desktop toasts and in-app banners. Off silences everything below.">
          <Switch checked={settings.notifications_enabled} onCheckedChange={v => patch({ notifications_enabled: v })} />
        </FieldRow>
        {settings.notifications_enabled && (
          <>
            <FieldRow label="Plan your day" description="Morning reminder to confirm today's working set on the Plan page.">
              <Switch checked={settings.notify_plan_nudge} onCheckedChange={v => patch({ notify_plan_nudge: v })} />
            </FieldRow>
            <FieldRow label="Worklog drafts ready" description="When the daily worklog drafts are ready to review and approve.">
              <Switch checked={settings.notify_worklog_ready} onCheckedChange={v => patch({ notify_worklog_ready: v })} />
            </FieldRow>
            <FieldRow label="System faults" description="When a tracker sync or the classifier stack fails (also shown as a banner).">
              <Switch checked={settings.notify_system_fault} onCheckedChange={v => patch({ notify_system_fault: v })} />
            </FieldRow>
            <FieldRow label="Quiet hours" description="Hold back desktop toasts during this window (banners still appear). Wraps past midnight.">
              <Switch checked={settings.quiet_hours_enabled} onCheckedChange={v => patch({ quiet_hours_enabled: v })} />
            </FieldRow>
            {settings.quiet_hours_enabled && (
              <FieldRow label="Window" description="From → to, local time.">
                <TextInput type="time" width={110} value={settings.quiet_hours_start} onChange={v => patch({ quiet_hours_start: v })} />
                <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>→</span>
                <TextInput type="time" width={110} value={settings.quiet_hours_end} onChange={v => patch({ quiet_hours_end: v })} />
              </FieldRow>
            )}
          </>
        )}
        <SaveButton
          status={notifStatus}
          onClick={() => save({
            notifications_enabled: settings.notifications_enabled,
            notify_plan_nudge: settings.notify_plan_nudge,
            notify_worklog_ready: settings.notify_worklog_ready,
            notify_system_fault: settings.notify_system_fault,
            quiet_hours_enabled: settings.quiet_hours_enabled,
            quiet_hours_start: settings.quiet_hours_start,
            quiet_hours_end: settings.quiet_hours_end,
          }, setNotifStatus)}
        />
      </SectionCard>
    </div>
  )
}
