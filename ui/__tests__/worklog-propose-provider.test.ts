//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Choosing which tracker a PROPOSED worklog ticket gets created on.
//
// The bug this covers: a proposal's provider is assigned at generate time as "the
// first configured tracker" (src/pm_worklog/generate.rs), so with Jira and GitHub
// both connected every new ticket was filed in Jira - and the card's corner icon,
// which reads the provider the worklog actually landed on, dutifully showed Jira
// for all of it. The fix is a picker; these pin when it may be shown and what the
// write is allowed to do.
//
// `canPickProposalProvider` is imported and exercised for real. The rest is scanned
// from source - this repo has no React render harness (see task-composer.test.ts).

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { canPickProposalProvider } from '../components/timeline/WorklogTargets'
import { trackerName } from '../lib/integrations'

const src = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')
const targets = src('components/timeline/WorklogTargets.tsx')
const worklog = src('components/timeline/useWorklog.ts')
const panel = src('components/timeline/DayTaskDetailPanel.tsx')
const column = src('components/timeline/DayTaskColumn.tsx')

const PROPOSE = { issue_type: 'Task', title: 'Ship it', description: 'do the thing' }

describe('canPickProposalProvider', () => {
  it('is true for a drafted proposal with two trackers connected', () => {
    expect(canPickProposalProvider({ propose: PROPOSE, state: 'drafted' }, 2)).toBe(true)
  })

  it('is false for a matched draft - each matched ticket carries its own board', () => {
    expect(canPickProposalProvider({ propose: null, state: 'drafted' }, 2)).toBe(false)
  })

  it('is false once approved or posted - the ticket may already exist', () => {
    expect(canPickProposalProvider({ propose: PROPOSE, state: 'approved' }, 2)).toBe(false)
    expect(canPickProposalProvider({ propose: PROPOSE, state: 'posted' }, 2)).toBe(false)
  })

  it('is false with one tracker - a choice of one is a label, not a decision', () => {
    expect(canPickProposalProvider({ propose: PROPOSE, state: 'drafted' }, 1)).toBe(false)
  })

  it('is false with no tracker connected', () => {
    expect(canPickProposalProvider({ propose: PROPOSE, state: 'drafted' }, 0)).toBe(false)
  })

  it('stays true as more trackers connect', () => {
    for (const n of [2, 3, 4, 5]) {
      expect(canPickProposalProvider({ propose: PROPOSE, state: 'drafted' }, n)).toBe(true)
    }
  })

  it('needs every condition, not any of them', () => {
    // Guards against the predicate degrading to an OR in a later edit.
    expect(canPickProposalProvider({ propose: null, state: 'approved' }, 5)).toBe(false)
    expect(canPickProposalProvider({ propose: null, state: 'drafted' }, 0)).toBe(false)
  })
})

describe('the non-pickable case still says where the ticket goes', () => {
  it('falls through to a statement naming the tracker', () => {
    expect(targets).toContain("{draft.created_task_key ? 'Created in' : 'Will be created in'}")
    expect(targets).toContain('trackerName(draft.provider)')
  })

  it('reads past tense once the ticket exists', () => {
    // "Will be created in Jira" beside a ticket that already exists is a lie the
    // user would reasonably act on.
    expect(targets).toContain('draft.created_task_key ?')
  })
})

describe('trackerName', () => {
  it('maps every known provider id to its display name', () => {
    expect(trackerName('jira')).toBe('Jira')
    expect(trackerName('github')).toBe('GitHub')
    expect(trackerName('linear')).toBe('Linear')
    expect(trackerName('trello')).toBe('Trello')
    expect(trackerName('azure_devops')).toBe('Azure DevOps')
  })

  it('falls back to the raw id rather than rendering an empty gap', () => {
    expect(trackerName('some_new_tracker')).toBe('some_new_tracker')
    expect(trackerName('')).toBe('')
  })
})

describe('the write goes through the store, never straight from the component', () => {
  it('the command lives in useWorklog, not in a component file', () => {
    expect(worklog).toContain("'set_worklog_provider'")
    expect(targets).not.toContain('set_worklog_provider')
    expect(panel).not.toContain('set_worklog_provider')
  })

  it('sends the day, task and provider - the daemon needs all three to find the row', () => {
    expect(/set_worklog_provider',\s*\{\s*day,\s*task_id: taskId,\s*provider,/.test(worklog)).toBe(true)
  })

  it('refuses to fire while a generate or approve is in flight', () => {
    // Re-pointing a draft mid-approve would race a ticket that is being created.
    const fn = worklog.slice(worklog.indexOf('function runSetProvider'))
    expect(fn).toContain("if (cur?.phase === 'approving' || cur?.phase === 'generating') return")
  })

  it('skips the round-trip when the provider is already selected', () => {
    const fn = worklog.slice(worklog.indexOf('function runSetProvider'))
    expect(fn).toContain('if (cur?.draft?.provider === provider) return')
  })

  it('renders from the draft the server returns, not an optimistic local patch', () => {
    // The server owns the rules (it can refuse); echoing the click locally would
    // show a choice that was never written.
    const fn = worklog.slice(worklog.indexOf('function runSetProvider'))
    expect(fn).toContain('patch(key, { draft: r, phase: \'idle\', loaded: true, error: r.error ?? null })')
  })

  it('cancels a pending post-confirm, which named the old board', () => {
    expect(worklog).toContain('setProvider: (provider: string) => {\n      setConfirming(false)')
  })
})

describe('the confirm names the board before an irreversible create', () => {
  it('says which tracker the new ticket lands in', () => {
    expect(panel).toContain('Create a new ${draft.propose.issue_type} in ${providerName(draft.provider)}')
  })
})

describe('the card corner icon follows the provider, and is never hardcoded', () => {
  it('reads posted_provider off the task rather than assuming one tracker', () => {
    expect(column).toContain('provider={laid.task.posted_provider}')
    for (const id of ['"jira"', "'jira'"]) expect(column).not.toContain(`<ProviderIcon provider=${id}`)
  })

  it('renders no pill at all when nothing has been posted yet', () => {
    // Better a missing pill than one claiming a board the work never reached.
    expect(column).toContain('laid.task.posted_provider &&')
  })
})
