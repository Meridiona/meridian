//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Advanced. Runtime-tuning knobs migrated 1:1 from the old
// SettingsView: Observability (OpenObserve export + log level, with the
// install/reload flow via useApplyObservability), ETL Pipeline poll interval,
// Session Classification thresholds, LLM local-model preference, and the
// Jira Updater toggle. These don't fit Integrations/Capture/Notifications/
// Appearance — they're internal daemon behavior, not user-facing product
// surfaces, so they're grouped here rather than invented a home for each.

'use client'

import { useState } from 'react'
import { Select } from '@/components/ui/Select'
import { Switch } from '@/components/ui/Switch'
import { NumberStepper } from '@/components/ui/NumberStepper'
import { TextInput } from '@/components/ui/TextInput'
import type { RuntimeSettings } from '@/lib/settings'
import { SectionCard, SectionHeader, FieldRow, SaveButton, SettingsButton, type SaveStatus } from './fields'
import { useApplyObservability } from './useApplyObservability'

const LOG_LEVEL_OPTIONS = [
  { value: 'DEBUG',   label: 'DEBUG' },
  { value: 'INFO',    label: 'INFO' },
  { value: 'WARNING', label: 'WARNING' },
  { value: 'ERROR',   label: 'ERROR' },
]

export function AdvancedSection({ settings, setSettings, patch, save }: {
  settings: RuntimeSettings
  setSettings: (s: RuntimeSettings) => void
  patch: (changes: Partial<RuntimeSettings>) => void
  save: (fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) => Promise<void>
}) {
  const [etlStatus, setEtlStatus] = useState<SaveStatus>('idle')
  const [classificationStatus, setClassificationStatus] = useState<SaveStatus>('idle')
  const [llmStatus, setLlmStatus] = useState<SaveStatus>('idle')
  const [jiraStatus, setJiraStatus] = useState<SaveStatus>('idle')
  const { reloadStatus, reloadMsg, apply: applyObservability } = useApplyObservability(settings, setSettings)
  const applying = reloadStatus === 'saving' || reloadStatus === 'installing' || reloadStatus === 'reloading'

  return (
    <div className="max-w-[640px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Runtime</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Advanced</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          Internal daemon behavior — tracing, classification thresholds, and the local model.
          Changes take effect on the next daemon tick unless noted.
        </p>
      </div>

      <SectionCard>
        <SectionHeader>Observability</SectionHeader>
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
              <TextInput type="email" value={settings.oo_email} onChange={v => patch({ oo_email: v })} placeholder="you@example.com" />
            </FieldRow>
            <FieldRow label="Password" description="Stored locally; used to log in at localhost:5080 and as auth for trace/log export.">
              <TextInput type="password" value={settings.oo_password} onChange={v => patch({ oo_password: v })} placeholder="••••••••" />
            </FieldRow>
            <FieldRow label="OTLP Endpoint (optional)" description="Advanced — leave blank for the local OpenObserve instance. Only set this to export to a remote collector.">
              <TextInput value={settings.otlp_endpoint} onChange={v => patch({ otlp_endpoint: v })} placeholder="http://localhost:5080/api/default/v1/traces" />
            </FieldRow>
          </>
        )}
        <div className="flex items-center gap-2.5 pt-2 flex-wrap" style={{ borderTop: '1px solid var(--t-hair)' }}>
          <SettingsButton onClick={applyObservability} disabled={applying}>
            {reloadStatus === 'saving' ? 'Saving…'
              : reloadStatus === 'installing' ? 'Installing…'
              : reloadStatus === 'reloading' ? 'Reloading…'
              : 'Apply'}
          </SettingsButton>
          {reloadStatus === 'done' && <span className="text-[12px]" style={{ color: 'var(--color-state-approved)' }}>Active</span>}
          {reloadStatus === 'error' && <span className="text-[12px]" style={{ color: 'var(--color-state-pending)' }}>{reloadMsg ?? 'Failed'}</span>}
          <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>
            {reloadStatus === 'installing' ? 'Downloading & installing OpenObserve (first time only)…'
              : reloadStatus === 'reloading' ? 'Restarting daemon…'
              : 'Apply handles everything — installs/starts/stops OpenObserve and restarts the daemon'}
          </span>
          {settings.otlp_enabled && (
            <SettingsButton variant="outline" onClick={() => {
              let base = 'http://localhost:5080'
              try {
                if (settings.otlp_endpoint) base = new URL(settings.otlp_endpoint).origin
              } catch { /* keep default */ }
              window.open(base, '_blank', 'noopener,noreferrer')
            }}>
              Open OpenObserve
            </SettingsButton>
          )}
        </div>
      </SectionCard>

      <SectionCard>
        <SectionHeader>ETL Pipeline</SectionHeader>
        <FieldRow label="Poll Interval" description="How often the ETL pipeline runs. Takes effect on the next tick.">
          <NumberStepper value={settings.poll_interval_secs} onChange={v => patch({ poll_interval_secs: v })} min={10} max={3600} step={10} />
          <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>sec</span>
        </FieldRow>
        <SaveButton status={etlStatus} onClick={() => save({ poll_interval_secs: settings.poll_interval_secs }, setEtlStatus)} />
      </SectionCard>

      <SectionCard>
        <SectionHeader>Session Classification</SectionHeader>
        <FieldRow label="Classification Enabled">
          <Switch checked={settings.classification_enabled} onCheckedChange={v => patch({ classification_enabled: v })} />
        </FieldRow>
        <FieldRow label="Min Session Duration" description="Sessions shorter than this are skipped by the classifier.">
          <NumberStepper value={settings.min_classification_duration_s} onChange={v => patch({ min_classification_duration_s: v })} min={1} />
          <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>sec</span>
        </FieldRow>
        <FieldRow label="Classification Timeout" description="Maximum time allowed per classification request.">
          <NumberStepper value={settings.classification_timeout_s} onChange={v => patch({ classification_timeout_s: v })} min={5} max={600} step={5} />
          <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>sec</span>
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
