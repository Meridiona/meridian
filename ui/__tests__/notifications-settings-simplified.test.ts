//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guards the simplified Settings → Notifications surface: a master switch and
// quiet hours only. The per-category toggles (plan nudge, worklog ready,
// system fault/pause/health/update, summariser digest, board hygiene) and the
// "Open planner each day" toggle were removed — every event is now gated
// solely by notifications_enabled + quiet hours, enforced at drain time in
// meridian-core (see meridian-core/src/notifications/mod.rs::event_allowed).

const uiRoot = import.meta.dir + '/..'
const readSrc = (rel: string): string => readFileSync(uiRoot + '/' + rel, 'utf8')

const REMOVED_SETTINGS_FIELDS = [
  'notify_plan_nudge',
  'notify_worklog_ready',
  'notify_system_fault',
  'notify_system_pause',
  'notify_system_health',
  'notify_system_update',
  'notify_summariser_digest',
  'notify_board_hygiene',
  'auto_open_plan',
]

describe('NotificationsSection', () => {
  const src = readSrc('components/timeline/settings/NotificationsSection.tsx')

  it('exposes only the master switch and quiet hours controls', () => {
    expect(src).toContain('notifications_enabled')
    expect(src).toContain('quiet_hours_enabled')
    expect(src).toContain('quiet_hours_start')
    expect(src).toContain('quiet_hours_end')
  })

  it('no longer renders any of the removed per-category toggles', () => {
    for (const field of REMOVED_SETTINGS_FIELDS) {
      expect(src).not.toContain(field)
    }
  })

  it('the master switch still gates everything below it, quiet hours included', () => {
    const gateIndex = src.indexOf('{settings.notifications_enabled && (')
    const quietHoursIndex = src.indexOf('quiet_hours_enabled')
    expect(gateIndex).toBeGreaterThan(-1)
    expect(quietHoursIndex).toBeGreaterThan(gateIndex)
  })
})

describe('RuntimeSettings (ui/lib/settings.ts)', () => {
  const src = readSrc('lib/settings.ts')

  it('keeps notifications_enabled and quiet_hours_* in the type and defaults', () => {
    expect(src).toContain('notifications_enabled: boolean')
    expect(src).toContain('notifications_enabled: true')
    expect(src).toContain('quiet_hours_enabled: boolean')
    expect(src).toContain('quiet_hours_enabled: false')
  })

  it('no longer declares any of the removed per-category settings fields', () => {
    for (const field of REMOVED_SETTINGS_FIELDS) {
      expect(src).not.toContain(field)
    }
  })
})
