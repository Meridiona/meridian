// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import type { RuntimeSettings } from '@/lib/settings'

type SaveStatus = 'idle' | 'saved' | 'error'

function Toggle({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      style={{
        position: 'relative',
        display: 'inline-flex',
        height: '20px',
        width: '36px',
        flexShrink: 0,
        cursor: 'pointer',
        borderRadius: '9999px',
        border: '2px solid transparent',
        transition: 'background-color 0.2s',
        background: checked ? 'var(--accent)' : 'var(--rule-2)',
      }}
    >
      <span
        style={{
          pointerEvents: 'none',
          display: 'inline-block',
          height: '16px',
          width: '16px',
          borderRadius: '9999px',
          background: 'var(--paper)',
          boxShadow: '0 1px 3px rgba(0,0,0,0.15)',
          transform: checked ? 'translateX(16px)' : 'translateX(0)',
          transition: 'transform 0.2s',
        }}
      />
    </button>
  )
}

function SectionCard({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ background: 'var(--surface)', border: '1px solid var(--rule)', borderRadius: '10px', padding: '20px', display: 'flex', flexDirection: 'column', gap: '16px' }}>
      {children}
    </div>
  )
}

function SectionHeader({ children }: { children: React.ReactNode }) {
  return (
    <p style={{ fontSize: '10px', fontWeight: 500, textTransform: 'uppercase', letterSpacing: '0.15em', color: 'var(--ink-3)' }}>
      {children}
    </p>
  )
}

function FieldRow({ label, description, children }: { label: string; description?: string; children: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between', gap: '24px' }}>
      <div style={{ minWidth: 0 }}>
        <p style={{ fontSize: '13px', fontWeight: 500, color: 'var(--ink)' }}>{label}</p>
        {description && <p style={{ fontSize: '11px', color: 'var(--ink-3)', marginTop: '2px' }}>{description}</p>}
      </div>
      <div style={{ flexShrink: 0 }}>{children}</div>
    </div>
  )
}

function NumberInput({ value, onChange, min, max, step }: { value: number; onChange: (v: number) => void; min?: number; max?: number; step?: number }) {
  return (
    <input
      type="number"
      value={value}
      min={min}
      max={max}
      step={step ?? 1}
      onChange={e => onChange(Number(e.target.value))}
      style={{
        border: '1px solid var(--rule)',
        borderRadius: '6px',
        padding: '5px 10px',
        fontSize: '13px',
        width: '90px',
        color: 'var(--ink)',
        background: 'var(--paper)',
        outline: 'none',
      }}
    />
  )
}

function SelectInput({ value, onChange, options }: { value: string; onChange: (v: string) => void; options: string[] }) {
  return (
    <select
      value={value}
      onChange={e => onChange(e.target.value)}
      style={{
        border: '1px solid var(--rule)',
        borderRadius: '6px',
        padding: '5px 10px',
        fontSize: '13px',
        color: 'var(--ink)',
        background: 'var(--paper)',
        outline: 'none',
      }}
    >
      {options.map(o => <option key={o} value={o}>{o}</option>)}
    </select>
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
          color: 'var(--paper)',
          fontSize: '12px',
          padding: '6px 14px',
          borderRadius: '6px',
          border: 'none',
          cursor: 'pointer',
        }}
      >
        Save
      </button>
      {status === 'saved' && <span style={{ fontSize: '12px', color: 'var(--success)' }}>Saved</span>}
      {status === 'error' && <span style={{ fontSize: '12px', color: 'var(--warn)' }}>Failed to save</span>}
    </div>
  )
}

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
        <FieldRow label="Log Level" description="Python agent log verbosity. DEBUG includes raw LLM prompts and rule hits.">
          <SelectInput
            value={settings.log_level}
            onChange={v => patch({ log_level: v as RuntimeSettings['log_level'] })}
            options={['DEBUG', 'INFO', 'WARNING', 'ERROR']}
          />
        </FieldRow>
        <SaveButton status={observabilityStatus} onClick={() => save({ log_level: settings.log_level }, setObservabilityStatus)} />
      </SectionCard>

      {/* ETL Pipeline */}
      <SectionCard>
        <SectionHeader>ETL Pipeline</SectionHeader>
        <FieldRow label="Poll Interval" description="How often the ETL pipeline runs. Takes effect on the next tick.">
          <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
            <NumberInput value={settings.poll_interval_secs} onChange={v => patch({ poll_interval_secs: v })} min={10} max={3600} />
            <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>sec</span>
          </div>
        </FieldRow>
        <SaveButton status={etlStatus} onClick={() => save({ poll_interval_secs: settings.poll_interval_secs }, setEtlStatus)} />
      </SectionCard>

      {/* Session Classification */}
      <SectionCard>
        <SectionHeader>Session Classification</SectionHeader>
        <FieldRow label="Classification Enabled">
          <Toggle checked={settings.classification_enabled} onChange={v => patch({ classification_enabled: v })} />
        </FieldRow>
        <FieldRow label="Min Session Duration" description="Sessions shorter than this are skipped by the classifier.">
          <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
            <NumberInput value={settings.min_classification_duration_s} onChange={v => patch({ min_classification_duration_s: v })} min={1} />
            <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>sec</span>
          </div>
        </FieldRow>
        <FieldRow label="Classification Timeout" description="Maximum time allowed per classification request.">
          <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
            <NumberInput value={settings.classification_timeout_s} onChange={v => patch({ classification_timeout_s: v })} min={5} max={600} />
            <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>sec</span>
          </div>
        </FieldRow>
        <FieldRow label="Auto-route Floor" description="Confidence above this → auto-link to task.">
          <NumberInput value={settings.agent_auto_floor} onChange={v => patch({ agent_auto_floor: v })} min={0} max={1} step={0.01} />
        </FieldRow>
        <FieldRow label="Queue Floor" description="Confidence above this → queue for review.">
          <NumberInput value={settings.agent_queue_floor} onChange={v => patch({ agent_queue_floor: v })} min={0} max={1} step={0.01} />
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
          <Toggle checked={settings.llm_prefer_local} onChange={v => patch({ llm_prefer_local: v })} />
        </FieldRow>
        <FieldRow label="Local Budget" description="Fraction of GPU headroom to allow.">
          <NumberInput value={settings.llm_budget_pct} onChange={v => patch({ llm_budget_pct: v })} min={0} max={1} step={0.05} />
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
          <Toggle checked={settings.jira_update_enabled} onChange={v => patch({ jira_update_enabled: v })} />
        </FieldRow>
        <SaveButton status={jiraStatus} onClick={() => save({ jira_update_enabled: settings.jira_update_enabled }, setJiraStatus)} />
      </SectionCard>
    </div>
  )
}
