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
  dev: 'var(--color-state-proposal)',
  staging: 'var(--color-state-pending)',
  prod: 'var(--t-faint)',
}

export function AccountSection() {
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
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Account</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Account</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          Re-run onboarding to reconfigure permissions, integrations, or the local model.
        </p>
      </div>

      <SectionCard>
        <SectionHeader>Account</SectionHeader>
        <AccountAuthControl
          onSignedIn={(email) => {
            invoke('save_account_email', { email }).catch(() => {})
          }}
          onSignedOut={() => {
            invoke('clear_account_email').catch(() => {})
          }}
        />
      </SectionCard>

      <SectionCard>
        <SectionHeader>Setup &amp; Onboarding</SectionHeader>
        <FieldRow label="Re-run Setup" description="Return to the onboarding wizard to reconfigure permissions, update integrations, or re-check the local model.">
          <SettingsButton onClick={() => {
            mutate('/api/setup', 'open_setup', {}).catch(err => console.error('Failed to open setup wizard', err))
          }}>
            Go to Setup
          </SettingsButton>
        </FieldRow>
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
        <FieldRow label="Support ID" description="Identifies you in error reports during alpha testing, so we can trace issues across your devices. Quote it when you contact support so we can find the errors from your account.">
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
