//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// A worklog can appear without this webview writing it: the daemon's end-of-day
// pass auto-generates drafts. The summary badge reads that ledger afresh, while the
// detail panel keeps a module-level store so an in-flight manual generation survives
// navigation. Those two otherwise diverge when the store cached `null` before the
// daemon wrote the draft: the list says DRAFT READY TO POST, but opening it says No
// update written yet until the user restarts Meridian.
//
// There is no React render harness in this repo, so pin the store contract at its
// decision point: every panel mount calls `ensureLoaded`, and a completed earlier
// read must not prevent a new database read. Active reads and writes still do.

import { describe, expect, it } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const hook = readFileSync(
  join(import.meta.dir, '..', 'components/timeline/useWorklog.ts'),
  'utf8',
)

function ensureLoaded(): string {
  const at = hook.indexOf('function ensureLoaded(')
  expect(at).toBeGreaterThan(-1)
  const end = hook.indexOf('\n}\n\n/** Run (or regenerate)', at)
  expect(end).toBeGreaterThan(at)
  return hook.slice(at, end)
}

describe('a draft generated in the background', () => {
  it('revalidates when the task detail mounts', () => {
    const effectAt = hook.indexOf('useEffect(() => {')
    const callAt = hook.indexOf('ensureLoaded(day, taskId)', effectAt)
    const depsAt = hook.indexOf('[day, taskId])', callAt)
    expect(effectAt).toBeGreaterThan(-1)
    expect(callAt).toBeGreaterThan(effectAt)
    expect(depsAt).toBeGreaterThan(callAt)
  })

  it('does not treat an earlier completed read as permanently authoritative', () => {
    const body = ensureLoaded()
    expect(body).not.toMatch(/e\.loaded/)
    expect(body).toContain("load<DayTaskWorklogDraft | null>(API, 'get_day_task_worklog'")
  })

  it('does not duplicate a read or clobber an active write', () => {
    const body = ensureLoaded()
    expect(body).toContain("e.phase === 'loading'")
    expect(body).toContain("e.phase === 'generating'")
    expect(body).toContain("e.phase === 'approving'")
  })

  it('replaces the cached null with the daemon-generated draft', () => {
    const body = ensureLoaded()
    expect(body).toContain('draft: r ?? null')
    expect(body).toContain("phase: 'idle'")
    expect(body).toContain('loaded: true')
  })
})
