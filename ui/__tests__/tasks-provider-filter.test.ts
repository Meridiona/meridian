//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { filterByConnectedProviders } from '../lib/integrations'
import type { IntegrationsResponse } from '../lib/api-types'

// Minimal task shape — only the field the filter inspects.
const task = (provider: string, today_s = 0) => ({ provider, today_s })

const ALL_DISCONNECTED: IntegrationsResponse = {
  jira: false, linear: false, github: false, trello: false, azure_devops: false,
  sync_errors: {},
}

const JIRA_ONLY: IntegrationsResponse = {
  ...ALL_DISCONNECTED,
  jira: true,
}

const JIRA_AND_LINEAR: IntegrationsResponse = {
  ...ALL_DISCONNECTED,
  jira: true,
  linear: true,
}

const ALL_CONNECTED: IntegrationsResponse = {
  jira: true, linear: true, github: true, trello: true, azure_devops: true,
  sync_errors: {},
}

// ---------------------------------------------------------------------------
// Core: disconnected provider tasks are hidden
// ---------------------------------------------------------------------------

describe('filterByConnectedProviders', () => {
  it('hides tasks from a disconnected provider', () => {
    const tasks = [task('jira'), task('github')]
    const result = filterByConnectedProviders(tasks, JIRA_ONLY)
    expect(result.map(t => t.provider)).toEqual(['jira'])
  })

  it('shows no tasks when all providers are disconnected', () => {
    const tasks = [task('jira'), task('github'), task('linear')]
    expect(filterByConnectedProviders(tasks, ALL_DISCONNECTED)).toEqual([])
  })

  it('shows all tasks when all providers are connected', () => {
    const tasks = [task('jira'), task('github'), task('linear'), task('trello'), task('azure_devops')]
    expect(filterByConnectedProviders(tasks, ALL_CONNECTED)).toHaveLength(5)
  })

  it('shows tasks from multiple connected providers and hides others', () => {
    const tasks = [task('jira'), task('linear'), task('github')]
    const result = filterByConnectedProviders(tasks, JIRA_AND_LINEAR)
    expect(result.map(t => t.provider).sort()).toEqual(['jira', 'linear'])
  })

  // ---------------------------------------------------------------------------
  // Loading state: integrations === null → no premature filtering
  // ---------------------------------------------------------------------------

  it('returns all tasks unchanged while integrations is loading (null)', () => {
    const tasks = [task('jira'), task('github'), task('trello')]
    expect(filterByConnectedProviders(tasks, null)).toEqual(tasks)
  })

  // ---------------------------------------------------------------------------
  // Downstream: provider tabs and touched count derived from filtered list
  // ---------------------------------------------------------------------------

  it('provider tabs derived from filtered list omit disconnected providers', () => {
    const tasks = [task('jira'), task('jira'), task('github')]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    const presentProviders = Array.from(new Set(active.map(t => t.provider))).sort()
    expect(presentProviders).toEqual(['jira'])
    expect(presentProviders).not.toContain('github')
  })

  it('showProviderTabs is false when only one connected provider has tasks', () => {
    const tasks = [task('jira'), task('jira'), task('github')]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    const presentProviders = Array.from(new Set(active.map(t => t.provider)))
    expect(presentProviders.length > 1).toBe(false)
  })

  it('touched-today count excludes tasks from disconnected providers', () => {
    // github task has today_s > 0 but github is not connected
    const tasks = [task('jira', 0), task('github', 3600)]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    const touched = active.filter(t => t.today_s > 0).length
    expect(touched).toBe(0)
  })

  it('touched-today count includes tasks from connected providers', () => {
    const tasks = [task('jira', 1800), task('github', 3600)]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    const touched = active.filter(t => t.today_s > 0).length
    expect(touched).toBe(1)
  })

  // ---------------------------------------------------------------------------
  // Edge cases
  // ---------------------------------------------------------------------------

  it('handles an empty task list gracefully', () => {
    expect(filterByConnectedProviders([], JIRA_ONLY)).toEqual([])
  })

  it('does not mutate the original task array', () => {
    const tasks = [task('jira'), task('github')]
    filterByConnectedProviders(tasks, JIRA_ONLY)
    expect(tasks).toHaveLength(2)
  })

  it('preserves all fields on retained tasks (not just provider)', () => {
    const full = { provider: 'jira', today_s: 120, key: 'JIR-1', title: 'Do stuff' }
    const result = filterByConnectedProviders([full], JIRA_ONLY)
    expect(result[0]).toEqual(full)
  })
})

// ---------------------------------------------------------------------------
// CleanupView: Board Score total and must-fix count exclude disconnected tasks
// ---------------------------------------------------------------------------
// These mirror the computations in CleanupView.tsx — total = filtered length,
// groups are built from the filtered list. If these break, the board score and
// must-fix badge would count tickets from disconnected integrations.

const mustFixHygiene = { bucket: 'must_fix', issues: [{ code: 'no_due_date', severity: 'must_fix', hint: 'Add a due date' }] }
const cleanHygiene   = { bucket: 'ready', issues: [] }

const hygieneTask = (provider: string, hygiene: typeof mustFixHygiene | typeof cleanHygiene) =>
  ({ provider, today_s: 0, hygiene })

describe('cleanup board: total and must-fix count filter disconnected providers', () => {
  it('total excludes tasks from disconnected providers', () => {
    const tasks = [hygieneTask('jira', cleanHygiene), hygieneTask('github', cleanHygiene)]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    expect(active.length).toBe(1) // github excluded
  })

  it('must-fix count excludes must-fix tasks from a disconnected provider', () => {
    const tasks = [hygieneTask('jira', cleanHygiene), hygieneTask('github', mustFixHygiene)]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    const mustCount = active.filter(t => t.hygiene?.issues.some(i => i.severity === 'must_fix')).length
    expect(mustCount).toBe(0) // github's must-fix doesn't count
  })

  it('board score is 100% when all connected-provider tasks are clean', () => {
    const tasks = [hygieneTask('jira', cleanHygiene), hygieneTask('github', mustFixHygiene)]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    const total = active.length
    const withIssues = active.filter(t => (t.hygiene?.issues.length ?? 0) > 0).length
    const ready = total - withIssues
    const score = total > 0 ? Math.round((ready / total) * 100) : 100
    expect(score).toBe(100)
  })

  it('board score is degraded only by connected-provider tasks with issues', () => {
    const tasks = [
      hygieneTask('jira', mustFixHygiene),  // connected, has issues
      hygieneTask('github', mustFixHygiene), // disconnected — must not affect score
    ]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    const total = active.length
    const withIssues = active.filter(t => (t.hygiene?.issues.length ?? 0) > 0).length
    const ready = total - withIssues
    const score = total > 0 ? Math.round((ready / total) * 100) : 100
    expect(score).toBe(0) // 0 ready out of 1 connected task
  })
})

// ---------------------------------------------------------------------------
// MustFixBanner: count excludes must-fix tasks from disconnected providers
// ---------------------------------------------------------------------------

describe('must-fix banner: count excludes disconnected providers', () => {
  it('banner count is 0 when only disconnected providers have must-fix tasks', () => {
    const tasks = [hygieneTask('github', mustFixHygiene), hygieneTask('linear', mustFixHygiene)]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    const count = active.filter(t => t.hygiene?.issues.some(i => i.severity === 'must_fix')).length
    expect(count).toBe(0)
  })

  it('banner count reflects only connected-provider must-fix tasks', () => {
    const tasks = [
      hygieneTask('jira', mustFixHygiene),   // connected + must-fix → counted
      hygieneTask('github', mustFixHygiene),  // disconnected → not counted
      hygieneTask('jira', cleanHygiene),      // connected + clean → not counted
    ]
    const active = filterByConnectedProviders(tasks, JIRA_ONLY)
    const count = active.filter(t => t.hygiene?.issues.some(i => i.severity === 'must_fix')).length
    expect(count).toBe(1)
  })

  it('banner count is 0 while integrations loads (null) and no tasks have must-fix', () => {
    const tasks = [hygieneTask('jira', cleanHygiene)]
    const active = filterByConnectedProviders(tasks, null)
    const count = active.filter(t => t.hygiene?.issues.some(i => i.severity === 'must_fix')).length
    expect(count).toBe(0)
  })
})

// ---------------------------------------------------------------------------
// MustFixBanner: suppress on the cleanup page (trailingSlash: true bug)
// ---------------------------------------------------------------------------
// With trailingSlash: true, usePathname() returns '/cleanup/' not '/cleanup'.
// The old check (=== '/cleanup') never matched, so the banner stayed visible
// on the very page where the user goes to fix things.

const shouldShowBanner = (count: number, pathname: string) =>
  !(count === 0 || pathname.startsWith('/cleanup'))

describe('must-fix banner: suppress on cleanup page', () => {
  it('hides when count is 0', () => {
    expect(shouldShowBanner(0, '/today')).toBe(false)
  })

  it('shows on non-cleanup pages when count > 0', () => {
    expect(shouldShowBanner(2, '/today')).toBe(true)
    expect(shouldShowBanner(2, '/tasks')).toBe(true)
  })

  it('hides on /cleanup (no trailing slash)', () => {
    expect(shouldShowBanner(2, '/cleanup')).toBe(false)
  })

  it('hides on /cleanup/ (trailing slash — the static-export path)', () => {
    expect(shouldShowBanner(2, '/cleanup/')).toBe(false)
  })

  it('hides on /cleanup sub-paths if they ever exist', () => {
    expect(shouldShowBanner(2, '/cleanup/detail')).toBe(false)
  })
})
