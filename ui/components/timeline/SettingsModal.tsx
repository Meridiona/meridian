//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings — sidebar + section router, ported from the Claude Design mock
// (Claude Design project b8656e29-ae04-4f69-b17f-d5fab4d00f3a, "Meridian
// Settings") onto the app's real --t-*/--color-state-* tokens and real data.
// Every feature the old flat SettingsView exposed is migrated 1:1 across the
// six sections below — nothing dropped, see each section file's header
// comment for its migration note. `initialSection` lets callers (the
// Toolbar's nav pill "Integrations" item) deep-link straight to a tab.

'use client'

import { useEffect, useState } from 'react'
import { load } from '@/lib/bridge'
import type { IntegrationsResponse } from '@/lib/api-types'
import { ModalShell } from './ModalShell'
import { SettingsSidebar } from './settings/SettingsSidebar'
import { IntegrationsSection } from './settings/IntegrationsSection'
import { WorklogSection } from './settings/WorklogSection'
import { IntelligenceSection } from './settings/IntelligenceSection'
import { CaptureSection } from './settings/CaptureSection'
import { NotificationsSection } from './settings/NotificationsSection'
import { AppearanceSection } from './settings/AppearanceSection'
// import { AdvancedSection } from './settings/AdvancedSection'
import { AccountSection } from './settings/AccountSection'
import { useRuntimeSettings } from './settings/useRuntimeSettings'
import { DEFAULT_SETTINGS_SECTION, type SettingsSection } from './settings/types'

export function SettingsModal({ onClose, initialSection }: {
  onClose: () => void
  initialSection?: SettingsSection
}) {
  const [section, setSection] = useState<SettingsSection>(initialSection ?? DEFAULT_SETTINGS_SECTION)
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  const [integrationsError, setIntegrationsError] = useState(false)
  const { settings, patch, save } = useRuntimeSettings()

  const fetchIntegrations = () => {
    load<IntegrationsResponse>('/api/integrations', 'get_integrations')
      .then(res => { setIntegrations(res); setIntegrationsError(false) })
      .catch(err => { console.error('Failed to load integrations', err); setIntegrationsError(true) })
  }
  useEffect(fetchIntegrations, [])

  return (
    <ModalShell title="Settings" onClose={onClose} maxWidth={980} scrollInside>
      <div className="flex flex-1 min-h-0">
        <SettingsSidebar section={section} onSelect={setSection} integrations={integrations} />
        <div className="flex-1 min-w-0 overflow-y-auto nice-scroll px-8 py-7">
          {!settings ? (
            <div className="flex flex-col gap-3 max-w-[640px]">
              {[1, 2, 3].map(i => (
                <div key={i} className="rounded-2xl h-28 bg-card" style={{ opacity: 0.5 }} />
              ))}
            </div>
          ) : (
            <>
              {section === 'integrations' && (
                integrationsError && !integrations ? (
                  <div className="flex flex-col items-start gap-2.5">
                    <p className="mt-body-sm" style={{ color: 'var(--status-error-dot)' }}>
                      Couldn't load integrations.
                    </p>
                    <button type="button" onClick={fetchIntegrations}
                      className="mt-body-sm px-3 py-1.5 rounded-md"
                      style={{ border: '1px solid var(--t-hair)', background: 'transparent', cursor: 'pointer' }}>
                      Retry
                    </button>
                  </div>
                ) : (
                  <IntegrationsSection integrations={integrations} onChanged={fetchIntegrations} />
                )
              )}
              {section === 'worklogs' && (
                <WorklogSection settings={settings} patch={patch} save={save} integrations={integrations} />
              )}
              {/* No `patch`: Intelligence stages its pick locally and writes only on Save. */}
              {section === 'intelligence' && <IntelligenceSection settings={settings} save={save} />}
              {section === 'capture' && <CaptureSection settings={settings} patch={patch} save={save} />}
              {section === 'notifications' && <NotificationsSection settings={settings} patch={patch} save={save} />}
              {section === 'appearance' && <AppearanceSection />}
              {/* {section === 'advanced' && (
                <AdvancedSection settings={settings} setSettings={setSettings} patch={patch} save={save} />
              )} */}
              {section === 'account' && <AccountSection />}
            </>
          )}
        </div>
      </div>
    </ModalShell>
  )
}
