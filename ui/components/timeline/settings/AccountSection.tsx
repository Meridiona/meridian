//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Account. Migrated 1:1 from the old SettingsView's "Setup &
// Onboarding" card — the only Account-ish control that existed before.

'use client'

import { useEffect, useState } from 'react'
import { load, mutate } from '@/lib/bridge'
import type { AppInfo } from '@/lib/api-types'
import { SectionCard, SectionHeader, FieldRow, SettingsButton } from './fields'

// Colored only for a non-prod build — a prod dashboard shouldn't draw the eye
// to its own version line; dev/staging should be unmistakable.
const CHANNEL_COLOR: Record<AppInfo['channel'], string> = {
  dev: 'var(--color-state-proposal)',
  staging: 'var(--color-state-pending)',
  prod: 'var(--t-faint)',
}

export function AccountSection() {
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null)

  useEffect(() => {
    load<AppInfo>('/api/app-info', 'get_app_info').then(setAppInfo).catch(() => {})
  }, [])

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
        <FieldRow label="Version" description="Which build of Meridian this is — dev, staging, or production.">
          {appInfo && (
            <span className="mt-body-sm font-mono font-semibold" style={{ color: CHANNEL_COLOR[appInfo.channel] }}>
              v{appInfo.version}{appInfo.channel !== 'prod' && ` · ${appInfo.channel}`}
            </span>
          )}
        </FieldRow>
      </SectionCard>
    </div>
  )
}
