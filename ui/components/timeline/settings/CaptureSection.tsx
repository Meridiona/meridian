//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Capture & Privacy. Also the home of the "Start Meridian
// automatically" switch (`autostart_enabled`) — see the comment above that card
// for why it cannot live in the hidden Advanced tab, and why it is now the only
// place autostart can be turned off at all.
//
// Work Hours — the scheduled capture
// auto-pause/resume window, migrated 1:1 from the old SettingsView's "Work
// Hours" card (manual pause/resume itself lives in the Toolbar's Capturing
// pill, unchanged — this section is only the SCHEDULE).
//
// Also hosts BOTH "what leaves my machine" consent switches, kept separate on
// purpose because they govern two different promises:
//
//   - "Error reporting" (`error_reporting_enabled`) — the redacted, error-only
//     telemetry a packaged install ships to Meridian's central OpenObserve, plus
//     Sentry crash reports. Pseudonymous (Support ID, never the account).
//   - "Product analytics" (`product_analytics_enabled`) — the daily heartbeat +
//     per-day count of product ACTIONS sent to PostHog under the account email
//     (see `meridian_core::usage_rollup` for exactly what is counted; it carries
//     no captured content).
//
// Conflating them would mean turning off crash reports silently kills usage
// reporting too, and vice versa. Both were drafted into `AdvancedSection.tsx`,
// but that tab is hidden — an opt-OUT default with an unreachable off-switch is
// not consent, so they live here. The setup wizard's completion note
// ("change in Settings", `ui/app/setup/steps.tsx`) points at these switches.

'use client'

import { useState } from 'react'
import { Switch } from '@/components/ui/Switch'
import { TextInput } from '@/components/ui/TextInput'
import type { RuntimeSettings } from '@/lib/settings'
import { SectionCard, SectionHeader, FieldRow, SaveButton, type SaveStatus } from './fields'
import { CaptureIgnoreCards } from './CaptureIgnoreCards'

export function CaptureSection({ settings, patch, save }: {
  settings: RuntimeSettings
  patch: (changes: Partial<RuntimeSettings>) => void
  save: (fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) => Promise<void>
}) {
  const [status, setStatus] = useState<SaveStatus>('idle')
  const [streamingStatus, setStreamingStatus] = useState<SaveStatus>('idle')
  const [secondaryMonitorsStatus, setSecondaryMonitorsStatus] = useState<SaveStatus>('idle')
  const [errorReportingStatus, setErrorReportingStatus] = useState<SaveStatus>('idle')
  const [productAnalyticsStatus, setProductAnalyticsStatus] = useState<SaveStatus>('idle')
  const [autostartStatus, setAutostartStatus] = useState<SaveStatus>('idle')

  return (
    <div className="max-w-[640px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--t-accent)' }}>Privacy</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Capture &amp; Privacy</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          Control when Meridian watches your screen. Everything stays on your Mac until you
          approve a work log.
        </p>
      </div>

      {/* Startup. Lives here rather than in the hidden AdvancedSection for the
          same reason the two consent switches do: an opt-OUT default whose
          off-switch is unreachable is not a choice. It is also the ONLY place
          this can be turned off now - the tray verifies and repairs its OS login
          job on every launch (see tray/src-tauri/src/autostart.rs), so switching
          the login item off in macOS System Settings no longer sticks. Writing
          this key registers or removes that job immediately. */}
      <SectionCard>
        <SectionHeader>Startup</SectionHeader>
        <FieldRow label="Start Meridian automatically" description="On by default: Meridian starts in your menu bar when you sign in, and comes back whenever you turn your machine on or wake it up. It opens quietly in the background - no window, nothing to dismiss. Meridian only records your work while it is running, so turning this off means days you forget to open it are not tracked.">
          <Switch checked={settings.autostart_enabled} onCheckedChange={v => patch({ autostart_enabled: v })} />
        </FieldRow>
        <SaveButton
          status={autostartStatus}
          onClick={() => save({ autostart_enabled: settings.autostart_enabled }, setAutostartStatus)}
        />
      </SectionCard>

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

      <SectionCard>
        <SectionHeader>Streaming video</SectionHeader>
        <FieldRow label="Skip streaming services" description="Netflix, Disney+, Hulu, Prime Video, Apple TV, Peacock, Paramount+, HBO Max, and Crunchyroll show a black screen to any screen recorder — including Meridian — so there is nothing to capture anyway. Enabling this skips those frames outright (in the app or in a browser tab) instead of storing a blank capture.">
          <Switch checked={settings.pause_on_streaming_video} onCheckedChange={v => patch({ pause_on_streaming_video: v })} />
        </FieldRow>
        <SaveButton
          status={streamingStatus}
          onClick={() => save({
            pause_on_streaming_video: settings.pause_on_streaming_video,
          }, setStreamingStatus)}
        />
      </SectionCard>

      <SectionCard>
        <SectionHeader>Multiple monitors</SectionHeader>
        <FieldRow label="Capture other monitors" description="On by default: Meridian glances at every connected monitor every so often, so work on a second screen shows up on your timeline whether or not it's the screen you're actively looking at. Turn this off if a second monitor may show things (a meeting, a personal window) you don't want tracked.">
          <Switch checked={settings.capture_secondary_monitors} onCheckedChange={v => patch({ capture_secondary_monitors: v })} />
        </FieldRow>
        <SaveButton
          status={secondaryMonitorsStatus}
          onClick={() => save({
            capture_secondary_monitors: settings.capture_secondary_monitors,
          }, setSecondaryMonitorsStatus)}
        />
      </SectionCard>

      <CaptureIgnoreCards settings={settings} patch={patch} save={save} />

      <SectionCard>
        <SectionHeader>Error reporting</SectionHeader>
        <FieldRow label="Send error reports" description="Meridian sends error-level logs to the team to help fix crashes and bugs. File paths, URLs, emails, and captured content are stripped on your device first - your screen activity, OCR text, and window titles are never sent. On by default; turn it off here any time.">
          <Switch checked={settings.error_reporting_enabled} onCheckedChange={v => patch({ error_reporting_enabled: v })} />
        </FieldRow>
        <SaveButton
          status={errorReportingStatus}
          onClick={() => save({
            error_reporting_enabled: settings.error_reporting_enabled,
          }, setErrorReportingStatus)}
        />
      </SectionCard>

      <SectionCard>
        <SectionHeader>Product analytics</SectionHeader>
        <FieldRow label="Send usage stats" description="Once a day Meridian sends a count of what it did for you - tickets updated, plans confirmed, summaries written, notifications delivered - plus a health snapshot: whether the daemon and your AI provider are working, which trackers are connected and syncing, and your notification and error-reporting settings. It's counts and status only: never your screen activity, window titles, ticket names, or anything you wrote. Sent under your account email. On by default; turn it off here any time.">
          <Switch checked={settings.product_analytics_enabled} onCheckedChange={v => patch({ product_analytics_enabled: v })} />
        </FieldRow>
        <SaveButton
          status={productAnalyticsStatus}
          onClick={() => save({
            product_analytics_enabled: settings.product_analytics_enabled,
          }, setProductAnalyticsStatus)}
        />
      </SectionCard>
    </div>
  )
}
