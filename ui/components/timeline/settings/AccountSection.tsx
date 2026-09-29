//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Account. Migrated 1:1 from the old SettingsView's "Setup &
// Onboarding" card — the only Account-ish control that existed before.
//
// Also hosts "Export Diagnostics" (moved from the now-hidden Advanced tab):
// the ONLY GUI path a packaged install has to hand a developer a local
// telemetry bundle (`meridian telemetry export` is the terminal fallback —
// not realistic mid support-ticket). See CLAUDE.md's "Telemetry: local-only
// capture, dev-only shipping" section — keep that doc's Settings path in
// sync if this ever moves again.

'use client'

import { useEffect, useState } from 'react'
import { invoke, load, mutate } from '@/lib/bridge'
import type { AppInfo } from '@/lib/api-types'
import { AccountAuthControl } from '@/app/setup/signin'
import { SectionCard, SectionHeader, FieldRow, SettingsButton } from './fields'
import { useExportDiagnostics } from './useExportDiagnostics'

// Colored only for a non-prod build — a prod dashboard shouldn't draw the eye
// to its own version line; dev/staging should be unmistakable.
const CHANNEL_COLOR: Record<AppInfo['channel'], string> = {
  dev: 'var(--t-accent)',
  staging: 'var(--color-state-pending)',
  prod: 'var(--t-faint)',
}

export function AccountSection({ onReplayTour, onReplayTourFromDay }: {
  /** Restart the first-run walkthrough. Supplied by the shell (which owns the
   *  walkthrough) and threaded through SettingsModal. */
  onReplayTour: () => void
  /** DEV ONLY, and null in a packaged build - see `TutorialHandle.replayFromDay`. */
  onReplayTourFromDay: (() => void) | null
}) {
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null)
  const [copied, setCopied] = useState(false)
  const { status: exportStatus, path: exportPath, errorMsg: exportError, exportBundle } = useExportDiagnostics()

  useEffect(() => {
    load<AppInfo>('/api/app-info', 'get_app_info').then(setAppInfo).catch(() => {})
  }, [])

  // A 16-hex string is error-prone to retype into a support email, and a
  // mistyped one silently matches no rows. Copy is the primary interaction.
  const copySupportId = async () => {
    if (!appInfo) return
    try {
      await navigator.clipboard.writeText(appInfo.supportId)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard blocked - the ID is rendered next to the button, so the
      // user can still select it by hand. Nothing to report.
    }
  }

  return (
    <div className="max-w-[640px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--t-accent)' }}>Account</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Account</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          Re-run setup to reconfigure permissions, integrations, or the local model.
        </p>
      </div>

      <SectionCard>
        <SectionHeader>Account</SectionHeader>
        <AccountAuthControl
          onSignedIn={(email) => {
            invoke('save_account_email', { email }).catch(() => {})
          }}
        />
      </SectionCard>

      <SectionCard>
        <SectionHeader>Setup &amp; Onboarding</SectionHeader>
        {/* Setup, NOT onboarding - and the copy says so, because the two used to
            be the same act. Finishing this wizard once armed the first-run
            walkthrough whatever the reason for opening it, so re-checking a
            permission entitled you to a tour you had already taken; it was then
            delivered on some later dashboard mount, typically an app update.
            Completing it from here now refreshes the onboarded stamp and nothing
            else (tray/src-tauri/src/commands/setup.rs). */}
        <FieldRow label="Re-run Setup" description="Return to the setup wizard to reconfigure permissions, update integrations, or re-check the local model. This will not replay the first-run walkthrough.">
          <SettingsButton onClick={() => {
            mutate('/api/setup', 'open_setup', {}).catch(err => console.error('Failed to open setup wizard', err))
          }}>
            Go to Setup
          </SettingsButton>
        </FieldRow>
        {/* The walkthrough is a once-ever surface, so without this the only way
            back to it is clearing a localStorage marker by hand. Closes Settings
            first — the walkthrough drives this very modal in two of its beats,
            so leaving it open would have it opening a modal already open.

            THIS SHIPS. It was `TOUR_DEV_TOOLS &&` on the argument that replaying
            is an authoring tool, with `window.__meridianTour()` named as the
            escape hatch for a packaged build. That escape hatch does not exist:
            the tray enables no `devtools` Cargo feature, so a release build has
            no inspector to type it into, and the dashboard window has no URL bar
            for `?tour=1` either. The gate did not make the replay developer-only,
            it made it UNREACHABLE - a walkthrough with no door, on every build a
            user actually runs.

            The tradeoff it was protecting against is real and stays real: part
            one runs on the user's own timeline, asks for a real daily plan and
            drives their real integrations screen (see `script.ts`'s "PART ONE").
            That is what the description below now says instead of promising an
            example day, and Skip is one click away from the first frame. */}
        <FieldRow label="Replay the walkthrough" description="Take the guided tour again. It starts on your own timeline and asks you to plan your day, the same as the first run, then moves to an example day for the rest. You can skip it at any point.">
          <SettingsButton onClick={onReplayTour}>
            Show me around
          </SettingsButton>
        </FieldRow>
        {/* DEV ONLY. Part one asks for a daily plan, a tracker connect (an OAuth
            round-trip through a browser) and possibly a CLI install before it
            reaches the example day - three or four minutes, most of it spent
            outside this window. Sitting through that on every edit is how the
            back half of the walkthrough stops getting edited, so there is a door
            straight into it. Absent entirely in a packaged build: it skips two
            REQUIRED steps, and a real user who took it would finish the tour
            with a Meridian that cannot write anything. */}
        {onReplayTourFromDay && (
          <FieldRow label="Skip to the example day"
            description="Dev only. Jumps past the plan and the connect steps, straight to the day rebuilding itself.">
            <SettingsButton onClick={onReplayTourFromDay}>
              Skip ahead
            </SettingsButton>
          </FieldRow>
        )}
      </SectionCard>

      <SectionCard>
        <SectionHeader>About</SectionHeader>
        <FieldRow label="Version" description="Which build of Meridian this is?">
          {appInfo && (
            <span className="mt-body-sm font-mono font-semibold" style={{ color: CHANNEL_COLOR[appInfo.channel] }}>
              v{appInfo.version}{appInfo.channel !== 'prod' && ` · ${appInfo.channel}`}
            </span>
          )}
        </FieldRow>
        <FieldRow
          label="Support ID"
          description={
            appInfo?.supportIdIsAccountScoped
              ? 'Identifies you in error reports during alpha testing, so we can trace issues across your devices. Quote it when you contact support so we can find the errors from your account.'
              : 'Identifies this device in error reports, without naming you or it. Quote it when you contact support so we can find the errors from this machine. It is not tied to your account and stays the same for this device.'
          }
        >
          {appInfo && (
            <div className="flex items-center gap-2">
              <span className="mt-body-sm font-mono" style={{ color: 'var(--t-muted)' }}>
                {appInfo.supportId}
              </span>
              <SettingsButton onClick={copySupportId}>
                {copied ? 'Copied' : 'Copy'}
              </SettingsButton>
            </div>
          )}
        </FieldRow>
        <FieldRow label="Export Diagnostics" description="Captures your local logs and traces for troubleshooting. Nothing leaves your machine until you share this file. Saved to your Downloads folder and revealed in your file manager.">
          <SettingsButton onClick={exportBundle} disabled={exportStatus === 'exporting'}>
            {exportStatus === 'exporting' ? 'Exporting…' : 'Export Diagnostics'}
          </SettingsButton>
        </FieldRow>
        {exportStatus === 'done' && exportPath && (
          <span className="text-[12px]" style={{ color: 'var(--color-state-approved)' }}>Saved to {exportPath}</span>
        )}
        {exportStatus === 'error' && (
          <span className="text-[12px]" style={{ color: 'var(--color-state-pending)' }}>{exportError ?? 'Export failed'}</span>
        )}
      </SectionCard>
    </div>
  )
}
