// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import Nav from '@/components/Nav'
import type { RuntimeSettings } from '@/lib/settings'

type SaveStatus = 'idle' | 'saved' | 'error'

function Toggle({
  checked,
  onChange,
}: {
  checked: boolean
  onChange: (v: boolean) => void
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className={[
        'relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent',
        'transition-colors duration-200 ease-in-out focus:outline-none focus-visible:ring-2 focus-visible:ring-[#141414]',
        checked ? 'bg-[#141414]' : 'bg-neutral-200',
      ].join(' ')}
    >
      <span
        className={[
          'pointer-events-none inline-block h-4 w-4 rounded-full bg-white shadow',
          'transform transition duration-200 ease-in-out',
          checked ? 'translate-x-4' : 'translate-x-0',
        ].join(' ')}
      />
    </button>
  )
}

function SectionCard({ children }: { children: React.ReactNode }) {
  return (
    <div className="bg-white border border-neutral-200 rounded-xl p-6 space-y-5">
      {children}
    </div>
  )
}

function SectionHeader({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-sm font-medium text-neutral-500 uppercase tracking-wide">
      {children}
    </p>
  )
}

function FieldRow({
  label,
  description,
  children,
}: {
  label: string
  description?: string
  children: React.ReactNode
}) {
  return (
    <div className="flex items-start justify-between gap-6">
      <div className="min-w-0">
        <p className="text-sm font-medium text-[#141414]">{label}</p>
        {description && (
          <p className="text-xs text-neutral-400 mt-0.5">{description}</p>
        )}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  )
}

function NumberInput({
  value,
  onChange,
  min,
  max,
  step,
}: {
  value: number
  onChange: (v: number) => void
  min?: number
  max?: number
  step?: number
}) {
  return (
    <input
      type="number"
      value={value}
      min={min}
      max={max}
      step={step ?? 1}
      onChange={e => onChange(Number(e.target.value))}
      className="border border-neutral-200 rounded-lg px-3 py-1.5 text-sm w-24 text-[#141414] bg-white focus:outline-none focus:border-[#141414] transition-colors"
    />
  )
}

function SaveButton({
  onClick,
  status,
}: {
  onClick: () => void
  status: SaveStatus
}) {
  return (
    <div className="flex items-center gap-3 pt-2 border-t border-neutral-100">
      <button
        type="button"
        onClick={onClick}
        className="bg-[#141414] text-white text-sm px-4 py-2 rounded-lg hover:bg-neutral-800 transition-colors"
      >
        Save
      </button>
      {status === 'saved' && (
        <span className="text-sm text-emerald-600">Saved</span>
      )}
      {status === 'error' && (
        <span className="text-sm text-red-500">Failed to save</span>
      )}
    </div>
  )
}

export default function SettingsPage() {
  const [settings, setSettings] = useState<RuntimeSettings | null>(null)
  const [observability, setObservability] = useState<SaveStatus>('idle')
  const [etl, setEtl] = useState<SaveStatus>('idle')
  const [classification, setClassification] = useState<SaveStatus>('idle')
  const [llm, setLlm] = useState<SaveStatus>('idle')
  const [jira, setJira] = useState<SaveStatus>('idle')

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

  async function save(
    fields: Partial<RuntimeSettings>,
    setStatus: (s: SaveStatus) => void,
  ) {
    setStatus('idle')
    try {
      const res = await fetch('/api/settings', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(fields),
      })
      if (!res.ok) throw new Error('non-ok response')
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
      <div className="min-h-screen bg-[#F8F7F4]">
        <Nav />
        <main className="max-w-2xl mx-auto px-5 py-10">
          <div className="space-y-3">
            {[1, 2, 3].map(i => (
              <div key={i} className="h-32 rounded-xl bg-neutral-100 animate-pulse" />
            ))}
          </div>
        </main>
      </div>
    )
  }

  return (
    <div className="min-h-screen bg-[#F8F7F4]">
      <Nav />
      <main className="max-w-2xl mx-auto px-5 py-10 space-y-6">
        <h1 className="text-2xl font-semibold tracking-tight text-[#141414]">Settings</h1>

        {/* Observability */}
        <SectionCard>
          <SectionHeader>Observability</SectionHeader>
          <FieldRow
            label="Log Level"
            description="Python agent log verbosity. DEBUG includes raw LLM prompts and rule hits."
          >
            <select
              value={settings.log_level}
              onChange={e =>
                patch({ log_level: e.target.value as RuntimeSettings['log_level'] })
              }
              className="border border-neutral-200 rounded-lg px-3 py-1.5 text-sm text-[#141414] bg-white focus:outline-none focus:border-[#141414] transition-colors"
            >
              <option value="DEBUG">DEBUG</option>
              <option value="INFO">INFO</option>
              <option value="WARNING">WARNING</option>
              <option value="ERROR">ERROR</option>
            </select>
          </FieldRow>
          <SaveButton
            status={observability}
            onClick={() => save({ log_level: settings.log_level }, setObservability)}
          />
        </SectionCard>

        {/* ETL Pipeline */}
        <SectionCard>
          <SectionHeader>ETL Pipeline</SectionHeader>
          <FieldRow
            label="Poll Interval"
            description="How often the ETL pipeline runs. Takes effect on the next tick."
          >
            <div className="flex items-center gap-2">
              <NumberInput
                value={settings.poll_interval_secs}
                onChange={v => patch({ poll_interval_secs: v })}
                min={10}
                max={3600}
              />
              <span className="text-xs text-neutral-400">sec</span>
            </div>
          </FieldRow>
          <SaveButton
            status={etl}
            onClick={() => save({ poll_interval_secs: settings.poll_interval_secs }, setEtl)}
          />
        </SectionCard>

        {/* Session Classification */}
        <SectionCard>
          <SectionHeader>Session Classification</SectionHeader>
          <FieldRow label="Classification Enabled">
            <Toggle
              checked={settings.classification_enabled}
              onChange={v => patch({ classification_enabled: v })}
            />
          </FieldRow>
          <FieldRow
            label="Min Session Duration"
            description="Sessions shorter than this are skipped by the classifier."
          >
            <div className="flex items-center gap-2">
              <NumberInput
                value={settings.min_classification_duration_s}
                onChange={v => patch({ min_classification_duration_s: v })}
                min={1}
              />
              <span className="text-xs text-neutral-400">sec</span>
            </div>
          </FieldRow>
          <FieldRow
            label="Classification Timeout"
            description="Maximum time allowed per classification request."
          >
            <div className="flex items-center gap-2">
              <NumberInput
                value={settings.classification_timeout_s}
                onChange={v => patch({ classification_timeout_s: v })}
                min={5}
                max={600}
              />
              <span className="text-xs text-neutral-400">sec</span>
            </div>
          </FieldRow>
          <FieldRow
            label="Auto-route Floor"
            description="Confidence above this → auto-link to task."
          >
            <NumberInput
              value={settings.agent_auto_floor}
              onChange={v => patch({ agent_auto_floor: v })}
              min={0}
              max={1}
              step={0.01}
            />
          </FieldRow>
          <FieldRow
            label="Queue Floor"
            description="Confidence above this → queue for review."
          >
            <NumberInput
              value={settings.agent_queue_floor}
              onChange={v => patch({ agent_queue_floor: v })}
              min={0}
              max={1}
              step={0.01}
            />
          </FieldRow>
          <SaveButton
            status={classification}
            onClick={() =>
              save(
                {
                  classification_enabled: settings.classification_enabled,
                  min_classification_duration_s: settings.min_classification_duration_s,
                  classification_timeout_s: settings.classification_timeout_s,
                  agent_auto_floor: settings.agent_auto_floor,
                  agent_queue_floor: settings.agent_queue_floor,
                },
                setClassification,
              )
            }
          />
        </SectionCard>

        {/* LLM */}
        <SectionCard>
          <SectionHeader>LLM</SectionHeader>
          <FieldRow
            label="Prefer Local Model"
            description="Use Apple Silicon MLX or local Ollama when available."
          >
            <Toggle
              checked={settings.llm_prefer_local}
              onChange={v => patch({ llm_prefer_local: v })}
            />
          </FieldRow>
          <FieldRow
            label="Local Budget"
            description="Fraction of GPU headroom to allow."
          >
            <NumberInput
              value={settings.llm_budget_pct}
              onChange={v => patch({ llm_budget_pct: v })}
              min={0}
              max={1}
              step={0.05}
            />
          </FieldRow>
          <SaveButton
            status={llm}
            onClick={() =>
              save(
                {
                  llm_prefer_local: settings.llm_prefer_local,
                  llm_budget_pct: settings.llm_budget_pct,
                },
                setLlm,
              )
            }
          />
        </SectionCard>

        {/* Jira Updater */}
        <SectionCard>
          <SectionHeader>Jira Updater</SectionHeader>
          <FieldRow label="Jira Updates Enabled">
            <Toggle
              checked={settings.jira_update_enabled}
              onChange={v => patch({ jira_update_enabled: v })}
            />
          </FieldRow>
          <SaveButton
            status={jira}
            onClick={() => save({ jira_update_enabled: settings.jira_update_enabled }, setJira)}
          />
        </SectionCard>
      </main>
    </div>
  )
}
