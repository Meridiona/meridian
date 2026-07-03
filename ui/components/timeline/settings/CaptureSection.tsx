//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Capture & Privacy. Work Hours — the scheduled capture
// auto-pause/resume window, migrated 1:1 from the old SettingsView's "Work
// Hours" card (manual pause/resume itself lives in the Toolbar's Capturing
// pill, unchanged — this section is only the SCHEDULE).

'use client'

import { useState } from 'react'
import { Switch } from '@/components/ui/Switch'
import { TextInput } from '@/components/ui/TextInput'
import type { RuntimeSettings } from '@/lib/settings'
import { SectionCard, SectionHeader, FieldRow, SaveButton, type SaveStatus } from './fields'

export function CaptureSection({ settings, patch, save }: {
  settings: RuntimeSettings
  patch: (changes: Partial<RuntimeSettings>) => void
  save: (fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) => Promise<void>
}) {
  const [status, setStatus] = useState<SaveStatus>('idle')

  return (
    <div className="max-w-[640px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Privacy</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Capture &amp; Privacy</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          Control when Meridian watches your screen. Everything stays on your Mac until you
          approve a work log.
        </p>
      </div>

      <SectionCard>
        <SectionHeader>Work hours</SectionHeader>
        <FieldRow label="Work hours" description="Meridian automatically pauses capture outside this window and resumes at the start of the next work session.">
          <Switch checked={settings.work_hours_enabled} onCheckedChange={v => patch({ work_hours_enabled: v })} />
        </FieldRow>
        {settings.work_hours_enabled && (
          <>
            <FieldRow label="Hours" description="Capture is active between these times (local time).">
              <TextInput type="time" width={110} value={settings.work_hours_start} onChange={v => patch({ work_hours_start: v })} />
              <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>→</span>
              <TextInput type="time" width={110} value={settings.work_hours_end} onChange={v => patch({ work_hours_end: v })} />
            </FieldRow>
            <FieldRow label="Days" description="Active capture days. Enter comma-separated numbers: 1=Mon … 7=Sun (e.g. '1,2,3,4,5').">
              <TextInput
                value={settings.work_days}
                onChange={v => patch({ work_days: v })}
                placeholder="1,2,3,4,5"
              />
            </FieldRow>
          </>
        )}
        <SaveButton
          status={status}
          onClick={() => save({
            work_hours_enabled: settings.work_hours_enabled,
            work_hours_start: settings.work_hours_start,
            work_hours_end: settings.work_hours_end,
            work_days: settings.work_days,
          }, setStatus)}
        />
      </SectionCard>
    </div>
  )
}
