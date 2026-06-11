//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState } from 'react'
import { Select } from '@/components/ui/Select'
import { Switch } from '@/components/ui/Switch'
import { NumberStepper } from '@/components/ui/NumberStepper'
import { TextInput } from '@/components/ui/TextInput'
import type { RuntimeSettings } from '@/lib/settings'

type SaveStatus = 'idle' | 'saved' | 'error'

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
          cursor: 'default',
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
  const [observabilityStatus, setObservabilityStatus] = useState<SaveStatus>('idle')
  const [etlStatus, setEtlStatus] = useState<SaveStatus>('idle')
  const [classificationStatus, setClassificationStatus] = useState<SaveStatus>('idle')
  const [llmStatus, setLlmStatus] = useState<SaveStatus>('idle')
  const [jiraStatus, setJiraStatus] = useState<SaveStatus>('idle')

  useEffect(() => {
    fetch('/api/settings')
      .then(r => r.json())
      .then((d: RuntimeSettings) => setSettings(d))
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
        <FieldRow label="Log Level" description="Controls verbosity of traces and logs sent to OpenObserve. DEBUG exports everything; WARNING/ERROR suppress info spans. Takes effect after a daemon restart.">
          <Select
            value={settings.log_level}
            onValueChange={v => patch({ log_level: v as RuntimeSettings['log_level'] })}
            options={LOG_LEVEL_OPTIONS}
          />
        </FieldRow>
        <FieldRow label="OpenObserve Export" description="Send traces and logs to the local OpenObserve instance. Requires credentials below; takes effect after a daemon restart.">
          <Switch checked={settings.otlp_enabled} onCheckedChange={v => patch({ otlp_enabled: v })} />
        </FieldRow>
        <FieldRow label="OTLP Endpoint" description="Leave blank to use the default (localhost:5080).">
          <TextInput
            value={settings.otlp_endpoint}
            onChange={v => patch({ otlp_endpoint: v })}
            placeholder="http://localhost:5080/api/default/v1/traces"
          />
        </FieldRow>
        <FieldRow label="Email">
          <TextInput
            type="email"
            value={settings.oo_email}
            onChange={v => patch({ oo_email: v })}
            placeholder="admin@example.com"
          />
        </FieldRow>
        <FieldRow label="Password">
          <TextInput
            type="password"
            value={settings.oo_password}
            onChange={v => patch({ oo_password: v })}
            placeholder="••••••••"
          />
        </FieldRow>
        <div style={{ display: 'flex', alignItems: 'center', gap: '10px', paddingTop: '8px', borderTop: '1px solid var(--rule)' }}>
          <button
            type="button"
            onClick={() => save({
              log_level: settings.log_level,
              otlp_enabled: settings.otlp_enabled,
              otlp_endpoint: settings.otlp_endpoint,
              oo_email: settings.oo_email,
              oo_password: settings.oo_password,
            }, setObservabilityStatus)}
            style={{
              background: 'var(--accent)',
              color: '#fff',
              fontSize: '12px',
              fontWeight: 500,
              padding: '5px 14px',
              borderRadius: '6px',
              border: 'none',
              cursor: 'default',
              boxShadow: '0 1px 3px rgba(0,0,0,0.15)',
            }}
          >
            Save
          </button>
          {observabilityStatus === 'saved' && <span style={{ fontSize: '12px', color: 'var(--success)' }}>Saved</span>}
          {observabilityStatus === 'error' && <span style={{ fontSize: '12px', color: 'var(--warn)' }}>Failed to save</span>}
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
    </div>
  )
}
