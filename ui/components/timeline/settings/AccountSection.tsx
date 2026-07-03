//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Account. Migrated 1:1 from the old SettingsView's "Setup &
// Onboarding" card — the only Account-ish control that existed before.

'use client'

import { mutate } from '@/lib/bridge'
import { SectionCard, SectionHeader, FieldRow, SettingsButton } from './fields'

export function AccountSection() {
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
          <SettingsButton onClick={() => { mutate('/api/setup', 'open_setup', {}).catch(() => {}) }}>
            Go to Setup
          </SettingsButton>
        </FieldRow>
      </SectionCard>
    </div>
  )
}
