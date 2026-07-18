//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Notifications. Migrated 1:1 from the old SettingsView's
// "Notifications" card: master switch, per-event-type toggles, and the quiet
// hours window.

'use client'

import { useState } from 'react'
import { Switch } from '@/components/ui/Switch'
import { TextInput } from '@/components/ui/TextInput'
import type { RuntimeSettings } from '@/lib/settings'
import { SectionCard, SectionHeader, FieldRow, SaveButton, type SaveStatus } from './fields'

export function NotificationsSection({ settings, patch, save }: {
  settings: RuntimeSettings
  patch: (changes: Partial<RuntimeSettings>) => void
  save: (fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) => Promise<void>
}) {
  const [status, setStatus] = useState<SaveStatus>('idle')

  return (
    <div className="max-w-[640px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Alerts</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Notifications</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          Choose what Meridian nudges you about, and when to stay quiet.
        </p>
      </div>

      <SectionCard>
        <SectionHeader>Delivery</SectionHeader>
        <FieldRow label="Notifications" description="Master switch for desktop toasts and in-app banners. Off silences everything below.">
          <Switch checked={settings.notifications_enabled} onCheckedChange={v => patch({ notifications_enabled: v })} />
        </FieldRow>
        {/* A window behaviour, not a toast — deliberately outside the
            notifications_enabled gate so silencing toasts doesn't also kill
            the morning planner. */}
        <FieldRow label="Open planner each day" description="Once a day, open the dashboard on the Plan view when you start using your machine.">
          <Switch checked={settings.auto_open_plan} onCheckedChange={v => patch({ auto_open_plan: v })} />
        </FieldRow>
        {settings.notifications_enabled && (
          <>
            <FieldRow label="Plan your day reminder" description="If you close the planner without confirming a plan, this reminds you an hour later.">
              <Switch checked={settings.notify_plan_nudge} onCheckedChange={v => patch({ notify_plan_nudge: v })} />
            </FieldRow>
            <FieldRow label="Worklog drafts ready" description="When the daily worklog drafts are ready to review and approve.">
              <Switch checked={settings.notify_worklog_ready} onCheckedChange={v => patch({ notify_worklog_ready: v })} />
            </FieldRow>
            <FieldRow label="System faults" description="When a tracker sync or the classifier stack fails (also shown as a banner).">
              <Switch checked={settings.notify_system_fault} onCheckedChange={v => patch({ notify_system_fault: v })} />
            </FieldRow>
            <FieldRow label="Pause / resume" description="When tracking is paused or resumes, manually or on a schedule.">
              <Switch checked={settings.notify_system_pause} onCheckedChange={v => patch({ notify_system_pause: v })} />
            </FieldRow>
            <FieldRow label="Daemon health" description="When Meridian's background daemon goes quiet or comes back online.">
              <Switch checked={settings.notify_system_health} onCheckedChange={v => patch({ notify_system_health: v })} />
            </FieldRow>
            <FieldRow label="Updates" description="When a new version is available, installing, or fails to install.">
              <Switch checked={settings.notify_system_update} onCheckedChange={v => patch({ notify_system_update: v })} />
            </FieldRow>
            <FieldRow label="Summarisation issues" description="A daily digest if any coding-agent sessions couldn't be summarised.">
              <Switch checked={settings.notify_summariser_digest} onCheckedChange={v => patch({ notify_summariser_digest: v })} />
            </FieldRow>
            <FieldRow label="Board hygiene" description="A daily digest if tickets on your board need attention.">
              <Switch checked={settings.notify_board_hygiene} onCheckedChange={v => patch({ notify_board_hygiene: v })} />
            </FieldRow>
            <FieldRow label="Quiet hours" description="Hold back desktop toasts during this window (banners still appear). Wraps past midnight.">
              <Switch checked={settings.quiet_hours_enabled} onCheckedChange={v => patch({ quiet_hours_enabled: v })} />
            </FieldRow>
            {settings.quiet_hours_enabled && (
              <FieldRow label="Window" description="From → to, local time.">
                <TextInput type="time" width={110} value={settings.quiet_hours_start} onChange={v => patch({ quiet_hours_start: v })} />
                <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>→</span>
                <TextInput type="time" width={110} value={settings.quiet_hours_end} onChange={v => patch({ quiet_hours_end: v })} />
              </FieldRow>
            )}
          </>
        )}
        <SaveButton
          status={status}
          onClick={() => save({
            notifications_enabled: settings.notifications_enabled,
            auto_open_plan: settings.auto_open_plan,
            notify_plan_nudge: settings.notify_plan_nudge,
            notify_worklog_ready: settings.notify_worklog_ready,
            notify_system_fault: settings.notify_system_fault,
            notify_system_pause: settings.notify_system_pause,
            notify_system_health: settings.notify_system_health,
            notify_system_update: settings.notify_system_update,
            notify_summariser_digest: settings.notify_summariser_digest,
            notify_board_hygiene: settings.notify_board_hygiene,
            quiet_hours_enabled: settings.quiet_hours_enabled,
            quiet_hours_start: settings.quiet_hours_start,
            quiet_hours_end: settings.quiet_hours_end,
          }, setStatus)}
        />
      </SectionCard>
    </div>
  )
}
