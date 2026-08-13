//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Every path that finishes a tracker connect must warm-start the ticket sync.
//
// # Why this needs a test
// The warm start is invisible when it works and invisible when it does not. Its
// entire payoff is that the first sync - a real round trip to Jira or Linear, tens
// of seconds on a first import - is spent while the user is still reading
// "Connected!" or picking projects, instead of on the planner staring at an empty
// board. Drop one of the call sites and nothing breaks, nothing errors, no test
// fails: the planner's own backstop still covers it, so the board still fills.
// The only symptom is that some connect paths feel slow again, which is precisely
// the kind of regression nobody traces back to a deleted line.
//
// Source-scanned rather than rendered, per the convention in task-composer.test.ts
// (there is no React render harness in this repo).

import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const read = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')

const CONNECT = read('components/IntegrationConnect.tsx')

describe('connect warm-starts the ticket sync', () => {
  test('it imports the shared helper', () => {
    expect(CONNECT).toContain("import { syncTasks } from '@/lib/taskSync'")
  })

  test('the one direct sync_tasks call is the explicit "Sync now" button, not a connect path', () => {
    // This file has exactly ONE bare `mutate(... 'sync_tasks' ...)` and it is
    // deliberate: `ProviderTasks`'s "Sync now" is a button the user pressed, so it
    // wants the REJECTION - it renders the specific failure text. `syncTasks` never
    // rejects (that is its contract), so routing this through the helper would trade
    // a real message for a generic one.
    //
    // Pinned as a count so a NEW direct call - the easy mistake, since it reads as
    // the obvious way to sync - fails here instead of quietly firing a second
    // outward request against the user's rate limit while a warm start is in flight.
    const direct = CONNECT.match(/'sync_tasks'/g) ?? []
    expect(direct.length).toBe(1)
    expect(CONNECT).toContain("setSyncError(typeof e === 'string'")
  })

  test('every connect-completion path starts it', () => {
    // Three today: the two project pickers' onSuccess, and TrackerSetup's `done`.
    // The count is asserted so that REMOVING one fails here even though the app
    // would still work.
    const calls = CONNECT.match(/syncTasks\(\)/g) ?? []
    expect(calls.length).toBe(3)
  })

  test('the picker paths sync before dismissing themselves', () => {
    // Order matters only for readability, but both pickers must have it at all.
    expect(CONNECT).toContain('<GitHubProjectPicker onSuccess={() => { void syncTasks()')
    expect(CONNECT).toContain('<JiraProjectPicker onSuccess={() => { void syncTasks()')
  })

  test('the token/OAuth completion path starts it too', () => {
    expect(CONNECT).toContain('const done = () => { void syncTasks()')
  })
})
