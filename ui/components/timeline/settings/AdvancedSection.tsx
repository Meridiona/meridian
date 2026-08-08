//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Advanced. Currently unreferenced — the nav entry + render
// branch are commented out in SettingsSidebar.tsx / SettingsModal.tsx, kept
// for easy re-enable. Runtime-tuning knobs migrated 1:1 from the old
// SettingsView: Observability (local capture + log level), ETL Pipeline poll
// interval, Session Classification thresholds, LLM local-model preference,
// and the Jira Updater toggle. These don't fit
// Integrations/Capture/Notifications/Appearance — they're internal daemon
// behavior, not user-facing product surfaces, so they're grouped here rather
// than invented a home for each.
//
// "Export Diagnostics" moved OUT of here to `AccountSection.tsx` (always
// reachable) when this tab was hidden — a packaged install's only GUI path
// to handing a developer a telemetry bundle can't live behind a hidden tab.
// Don't re-add it here if this section is ever re-enabled; it would just
// duplicate the Account one.

'use client'

import { useState } from 'react'
import { Select } from '@/components/ui/Select'
import { Switch } from '@/components/ui/Switch'
import { NumberStepper } from '@/components/ui/NumberStepper'
import type { RuntimeSettings } from '@/lib/settings'
import { SectionCard, SectionHeader, FieldRow, SaveButton, type SaveStatus } from './fields'

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
  const [logLevelStatus, setLogLevelStatus] = useState<SaveStatus>('idle')

  return (
    <div className="max-w-[640px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--t-accent)' }}>Runtime</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Advanced</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          Internal daemon behavior — tracing, classification thresholds, and the local model.
          Changes take effect on the next daemon tick unless noted.
        </p>
      </div>

      <SectionCard>
        <SectionHeader>Observability</SectionHeader>
        <FieldRow label="Log Level" description="Verbosity of local logs and traces. DEBUG logs everything; WARNING/ERROR suppress info. Hot-reloads on the next daemon tick.">
          <Select
            value={settings.log_level}
            onValueChange={v => patch({ log_level: v as RuntimeSettings['log_level'] })}
            options={LOG_LEVEL_OPTIONS}
          />
        </FieldRow>
        <SaveButton status={logLevelStatus} onClick={() => save({ log_level: settings.log_level }, setLogLevelStatus)} />
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
        <SectionHeader>Task Routing</SectionHeader>
        <FieldRow label="Auto-route Floor" description="Confidence above this → auto-link to task.">
          <NumberStepper value={settings.agent_auto_floor} onChange={v => patch({ agent_auto_floor: v })} min={0} max={1} step={0.05} />
        </FieldRow>
        <FieldRow label="Queue Floor" description="Confidence above this → queue for review.">
          <NumberStepper value={settings.agent_queue_floor} onChange={v => patch({ agent_queue_floor: v })} min={0} max={1} step={0.05} />
        </FieldRow>
        <SaveButton
          status={classificationStatus}
          onClick={() => save({
            agent_auto_floor: settings.agent_auto_floor,
            agent_queue_floor: settings.agent_queue_floor,
          }, setClassificationStatus)}
        />
      </SectionCard>

      <SectionCard>
        <SectionHeader>LLM</SectionHeader>
        {/* "Prefer Local Model" lived here and was read by nobody - the daemon never
            consulted llm_prefer_local, so toggling and saving it did nothing at all.
            Which AI runs the pipeline is now settings.llm_provider, chosen in the setup
            wizard and switchable from the Intelligence section. */}
        <FieldRow label="Local Budget" description="Fraction of GPU headroom to allow.">
          <NumberStepper value={settings.llm_budget_pct} onChange={v => patch({ llm_budget_pct: v })} min={0} max={1} step={0.05} />
        </FieldRow>
        <SaveButton
          status={llmStatus}
          onClick={() => save({ llm_budget_pct: settings.llm_budget_pct }, setLlmStatus)}
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
