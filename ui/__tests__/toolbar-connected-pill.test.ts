//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The toolbar's Connected/Solo pill, after it went from showing at most ONE brand
// mark (only when exactly one tracker was connected) to showing every connected
// provider's mark.
//
// The derivation in useTimelineData is mirrored here - it lives inside a hook body
// and this repo has no React render harness (see task-composer.test.ts) - and the
// rendering rules are pinned by scanning the source, so a regression in either half
// fails rather than passing on the other's correctness.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { TRACKERS, TRACKER_BY_ID, type TrackerId } from '../lib/integrations'
import type { IntegrationsResponse } from '../lib/api-types'

const src = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')
const toolbar = src('components/timeline/Toolbar.tsx')
const data = src('components/timeline/useTimelineData.ts')

const PROVIDER_IDS: TrackerId[] = ['jira', 'linear', 'github', 'trello', 'azure_devops']

/** An integrations response with `on` connected and everything else off. */
const integrations = (on: TrackerId[]): IntegrationsResponse => ({
  jira: on.includes('jira'),
  linear: on.includes('linear'),
  github: on.includes('github'),
  trello: on.includes('trello'),
  azure_devops: on.includes('azure_devops'),
  github_projects_selected: false,
  jira_projects_selected: false,
  sync_errors: {},
})

/** The hook's derivation, mirrored. The source is the contract; this pins its shape. */
function pill(int: IntegrationsResponse | null) {
  if (!int) return { isSolo: false, connectedProviderName: null, connectedProviderIds: [] }
  const on = PROVIDER_IDS.filter(id => (int as unknown as Record<string, boolean>)[id])
  return {
    isSolo: on.length === 0,
    connectedProviderName: on.length === 1 ? TRACKER_BY_ID[on[0]].name : null,
    connectedProviderIds: on,
  }
}

const label = (p: ReturnType<typeof pill>) =>
  p.isSolo ? 'Solo' : p.connectedProviderName ?? 'Connected'

describe('the pill with nothing connected', () => {
  it('reads Solo and offers no marks', () => {
    const p = pill(integrations([]))
    expect(p.isSolo).toBe(true)
    expect(p.connectedProviderIds).toEqual([])
    expect(label(p)).toBe('Solo')
  })
})

describe('the pill with one tracker', () => {
  it('names that tracker and shows its mark', () => {
    const p = pill(integrations(['jira']))
    expect(p.connectedProviderIds).toEqual(['jira'])
    expect(label(p)).toBe('Jira')
    expect(p.isSolo).toBe(false)
  })

  it('works for every provider, not just the first in the list', () => {
    for (const t of TRACKERS) {
      const p = pill(integrations([t.id]))
      expect(p.connectedProviderIds).toEqual([t.id])
      expect(label(p)).toBe(t.name)
    }
  })
})

describe('the pill with several trackers - the case that used to render one dot', () => {
  it('lists every connected provider', () => {
    expect(pill(integrations(['jira', 'github'])).connectedProviderIds).toEqual(['jira', 'github'])
  })

  it('falls back to a generic label, since no single name is right', () => {
    expect(label(pill(integrations(['jira', 'github'])))).toBe('Connected')
  })

  it('handles all five at once', () => {
    const p = pill(integrations([...PROVIDER_IDS]))
    expect(p.connectedProviderIds).toHaveLength(5)
    expect(label(p)).toBe('Connected')
  })

  it('orders marks by the registry, not by the response key order', () => {
    // The Settings banner ("Connected to Jira, GitHub") derives from the same
    // registry order - the two surfaces must not disagree about the same set.
    expect(pill(integrations(['github', 'jira'])).connectedProviderIds).toEqual(['jira', 'github'])
  })
})

describe('the pill while integrations are still loading', () => {
  it('is neither Solo nor connected - no flash of a wrong state', () => {
    const p = pill(null)
    expect(p.isSolo).toBe(false)
    expect(p.connectedProviderIds).toEqual([])
    expect(label(p)).toBe('Connected')
  })
})

describe('the source backs the derivation', () => {
  it('exposes the full id list, not a single id', () => {
    expect(data).toContain('connectedProviderIds: on,')
    expect(data).not.toContain('connectedProviderId:')
  })

  it('still names a single tracker only when exactly one is connected', () => {
    expect(data).toContain("connectedProviderName: on.length === 1 ? TRACKER_BY_ID[on[0]].name : null")
  })

  it('renders one mark per connected provider', () => {
    expect(toolbar).toContain('connectedProviderIds.map(id => <ProviderIcon key={id} provider={id} size={12} />)')
  })

  it('keys the marks, so React does not reorder them on a connect/disconnect', () => {
    expect(toolbar).toContain('key={id}')
  })

  it('keeps the dot fallback for the no-marks case', () => {
    expect(toolbar).toContain('connectedProviderIds.length > 0')
    expect(toolbar).toContain("background: isSolo ? 'var(--t-faint)' : 'var(--t-accent)'")
  })

  it('is display-only - the pill is not a toggle', () => {
    const pillBlock = toolbar.slice(toolbar.indexOf('connectedProviderIds.length > 0') - 400)
    expect(pillBlock.slice(0, 600)).not.toContain('onClick')
  })
})
