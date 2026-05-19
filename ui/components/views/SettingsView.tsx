// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import type { RuntimeSettings } from '@/lib/settings'

type SaveStatus = 'idle' | 'saved' | 'error'

// ── Apple-style Toggle ───────────────────────────────────────────────────────
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
        height: '22px',
        width: '40px',
        flexShrink: 0,
        cursor: 'default',
        borderRadius: '11px',
        border: 'none',
        transition: 'background 0.25s',
        background: checked ? 'var(--accent)' : 'rgba(120,120,128,0.32)',
        boxShadow: '0 1px 3px rgba(0,0,0,0.18) inset',
      }}
    >
      <span style={{
        pointerEvents: 'none',
        position: 'absolute',
        top: '2px',
        left: checked ? '20px' : '2px',
        width: '18px',
        height: '18px',
        borderRadius: '50%',
        background: '#fff',
        boxShadow: '0 1px 4px rgba(0,0,0,0.25)',
        transition: 'left 0.2s cubic-bezier(0.25,0.46,0.45,0.94)',
      }} />
    </button>
  )
}

// ── Apple macOS-style Select ─────────────────────────────────────────────────
function SelectInput({ value, onChange, options }: { value: string; onChange: (v: string) => void; options: string[] }) {
  const [focused, setFocused] = useState(false)
  return (
    <div style={{
      display: 'inline-block',
      position: 'relative',
      borderRadius: '5px',
      border: focused ? '1px solid var(--accent)' : '1px solid rgba(0,0,0,0.14)',
      boxShadow: focused
        ? '0 0 0 3px rgba(0,122,255,0.25)'
        : '0 0.5px 2px rgba(0,0,0,0.12)',
      background: 'var(--surface)',
      transition: 'box-shadow 0.15s, border-color 0.15s',
    }}>
      <select
        value={value}
        onChange={e => onChange(e.target.value)}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        style={{
          WebkitAppearance: 'none',
          appearance: 'none',
          background: 'transparent',
          border: 'none',
          outline: 'none',
          borderRadius: '5px',
          padding: '5px 30px 5px 9px',
          fontSize: '13px',
          color: 'var(--ink)',
          cursor: 'default',
          minWidth: '100px',
        }}
      >
        {options.map(o => <option key={o} value={o}>{o}</option>)}
      </select>
      {/* macOS-style blue badge with up/down chevron */}
      <span style={{
        position: 'absolute',
        right: 0,
        top: 0,
        bottom: 0,
        width: '22px',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'var(--accent)',
        borderRadius: '0 4px 4px 0',
        pointerEvents: 'none',
      }}>
        <svg viewBox="0 0 10 16" width="8" height="12" fill="white">
          <path d="M5 4L1 8h8L5 4zm0 8L1 8h8l-4 4z" />
        </svg>
      </span>
    </div>
  )
}

// ── Apple macOS-style Stepper ────────────────────────────────────────────────
function NumberInput({ value, onChange, min, max, step }: {
  value: number; onChange: (v: number) => void; min?: number; max?: number; step?: number
}) {
  const s = step ?? 1
  const atMin = min !== undefined && value <= min
  const atMax = max !== undefined && value >= max

  function dec() { onChange(min !== undefined ? Math.max(min, value - s) : value - s) }
  function inc() { onChange(max !== undefined ? Math.min(max, value + s) : value + s) }

  const btnBase: React.CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: '28px',
    height: '28px',
    border: 'none',
    background: 'transparent',
    cursor: 'default',
    transition: 'background 0.1s',
    flexShrink: 0,
  }

  return (
    <div style={{
      display: 'inline-flex',
      alignItems: 'stretch',
      border: '1px solid rgba(0,0,0,0.14)',
      borderRadius: '5px',
      boxShadow: '0 0.5px 2px rgba(0,0,0,0.12)',
      overflow: 'hidden',
      background: 'var(--surface)',
      height: '28px',
    }}>
      <button
        type="button"
        disabled={atMin}
        onClick={dec}
        style={{ ...btnBase, opacity: atMin ? 0.3 : 1, color: 'var(--ink)' }}
      >
        <svg viewBox="0 0 10 2" width="10" height="2">
          <line x1="1" y1="1" x2="9" y2="1" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
        </svg>
      </button>

      <span style={{ width: '1px', background: 'rgba(0,0,0,0.1)', flexShrink: 0 }} />

      <input
        type="number"
        value={value}
        min={min}
        max={max}
        step={s}
        onChange={e => {
          const n = Number(e.target.value)
          if (isNaN(n)) return
          const clamped = min !== undefined && max !== undefined
            ? Math.min(max, Math.max(min, n))
            : min !== undefined ? Math.max(min, n)
            : max !== undefined ? Math.min(max, n) : n
          onChange(clamped)
        }}
        className="mac-stepper-input"
        style={{
          border: 'none',
          background: 'transparent',
          outline: 'none',
          fontSize: '13px',
          color: 'var(--ink)',
          textAlign: 'center',
          width: '52px',
          padding: '0',
          height: '100%',
        }}
      />

      <span style={{ width: '1px', background: 'rgba(0,0,0,0.1)', flexShrink: 0 }} />

      <button
        type="button"
        disabled={atMax}
        onClick={inc}
        style={{ ...btnBase, opacity: atMax ? 0.3 : 1, color: 'var(--ink)' }}
      >
        <svg viewBox="0 0 10 10" width="10" height="10">
          <line x1="5" y1="1" x2="5" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          <line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
        </svg>
      </button>
    </div>
  )
}

// ── Layout helpers ────────────────────────────────────────────────────────────
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
      <div style={{ flexShrink: 0, display: 'flex', alignItems: 'center', gap: '6px' }}>{children}</div>
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
          borderRadius: '5px',
          border: 'none',
          cursor: 'default',
          boxShadow: '0 0.5px 2px rgba(0,0,0,0.18)',
        }}
      >
        Save
      </button>
      {status === 'saved' && <span style={{ fontSize: '12px', color: 'var(--success)' }}>Saved</span>}
      {status === 'error' && <span style={{ fontSize: '12px', color: 'var(--warn)' }}>Failed to save</span>}
    </div>
  )
}

// ── Main view ────────────────────────────────────────────────────────────────
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
    <>
      {/* Hide native number spinners */}
      <style>{`.mac-stepper-input::-webkit-inner-spin-button,.mac-stepper-input::-webkit-outer-spin-button{-webkit-appearance:none;margin:0}.mac-stepper-input{-moz-appearance:textfield}`}</style>

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
            <NumberInput value={settings.poll_interval_secs} onChange={v => patch({ poll_interval_secs: v })} min={10} max={3600} step={10} />
            <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>sec</span>
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
            <NumberInput value={settings.min_classification_duration_s} onChange={v => patch({ min_classification_duration_s: v })} min={1} />
            <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>sec</span>
          </FieldRow>
          <FieldRow label="Classification Timeout" description="Maximum time allowed per classification request.">
            <NumberInput value={settings.classification_timeout_s} onChange={v => patch({ classification_timeout_s: v })} min={5} max={600} step={5} />
            <span style={{ fontSize: '11px', color: 'var(--ink-3)' }}>sec</span>
          </FieldRow>
          <FieldRow label="Auto-route Floor" description="Confidence above this → auto-link to task.">
            <NumberInput value={settings.agent_auto_floor} onChange={v => patch({ agent_auto_floor: v })} min={0} max={1} step={0.05} />
          </FieldRow>
          <FieldRow label="Queue Floor" description="Confidence above this → queue for review.">
            <NumberInput value={settings.agent_queue_floor} onChange={v => patch({ agent_queue_floor: v })} min={0} max={1} step={0.05} />
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
    </>
  )
}
