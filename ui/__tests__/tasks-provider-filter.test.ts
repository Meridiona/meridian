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
