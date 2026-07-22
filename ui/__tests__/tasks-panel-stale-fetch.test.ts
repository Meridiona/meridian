//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guard for a stale-response race in ProviderTasks (IntegrationConnect.tsx):
// the lazy load on panel-expand and the sync-triggered reload both call
// fetchTasks(), and can overlap. Without a generation guard, a slower older
// response landing after a newer one would silently overwrite fresher data
// with stale data. The repo has no React render harness (see
// oauth-setup-lifecycle.test.ts), so this asserts the guard's shape directly
// against the source.
const uiRoot = import.meta.dir + '/..'
const view = readFileSync(uiRoot + '/components/IntegrationConnect.tsx', 'utf8')

describe('ProviderTasks.fetchTasks stale-response guard', () => {
  it('stamps each fetch with a generation counter', () => {
    expect(view).toMatch(/const fetchGen = useRef\(0\)/)
    expect(view).toMatch(/const gen = \+\+fetchGen\.current/)
  })

  it('only applies a response if it is still the latest generation', () => {
    expect(view).toMatch(/if \(gen === fetchGen\.current\) setTasks\(/)
    expect(view).toMatch(/if \(gen === fetchGen\.current\) setLoadError\(/)
  })
})
