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
// The worklog surfaces moved out of the panel into a dialog of their own; the
// confirm step lives with the other phase controls.
const actions = src('components/timeline/WorklogActions.tsx')
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
    // An unresolved provider is the one id that has no readable fallback of its
    // own - returning it verbatim put a hole in every sentence built on this
    // ("Will be created in ").
    expect(trackerName('')).toBe('your tracker')
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
    expect(actions).toContain('Create a new ${draft.propose.issue_type} in ${providerName(draft.provider)}')
  })
})

describe('the draft names the ticket, not the delivery mechanism', () => {
  const targets = src('components/timeline/WorklogTargets.tsx')

  it('leads with the ticket title, and demotes key + tracker + match to meta', () => {
    // The row opened "Comment on MER-475 · 90% match" in bold with the ticket's
    // actual name underneath it, smaller and dimmer - so the largest text
    // described HOW the update is delivered, while the one thing the user has to
    // judge ("is this the right ticket?") was the quietest thing in the box.
    expect(targets).not.toContain("'Comment on'")
    expect(targets).toContain('{task_title || task_key}')
    // The tracker is a mark, not a word: recognised faster than it is read.
    expect(targets).toContain('<ProviderIcon provider={provider}')
  })
})

describe('a posted update is stated once', () => {
  const actions = src('components/timeline/WorklogActions.tsx')

  it('does not print "Posted to X" beside "Linked to X"', () => {
    // The same fact in two shapes with two verbs reads as two different things
    // having happened, and leaves the user working out which one to click. The
    // outcome is said once and the ticket itself is the link.
    // Code lines only - the comment above the fix quotes the old string.
    const code = actions.split('\n').filter(l => !/^\s*(\/\/|\*|\/\*)/.test(l)).join('\n')
    expect(code).not.toContain('Linked to')
    expect(code).toContain('function TicketLink(')
    expect(actions).toContain('openExternal(url)')
  })

  it('drops the chip it replaced rather than leaving it behind', () => {
    // LinkChip existed only for that duplicate row; left exported it is dead code
    // that reads like the sanctioned way to render a ticket link.
    const kit = src('components/timeline/dayTaskKit.tsx')
    expect(kit).not.toContain('LinkChip')
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

describe('the update is a document with edges and a name', () => {
  const dialog = src('components/timeline/WorklogDraftDialog.tsx')
  const summary = src('components/tutorial/TutorialSummaryTask.tsx')

  it('puts the target and the update in ONE card, for both callers', () => {
    // They were two stacked panels in two tints - an amber proposal above a
    // lilac update box - which said, in the only language a layout has, that
    // they were separate things to weigh. They are one object: a ticket and the
    // text that goes on it. Both top-level surfaces render the same component,
    // so the walkthrough teaches the screen that actually ships.
    expect(dialog).toContain('<DraftDocument draft={draft}')
    expect(summary).toContain('<DraftDocument draft={draft}')
    expect(targets).toContain('export function DraftDocument')
    // Nested inside that card, the parts carry no frame of their own.
    expect(targets).toContain('<UpdateBody update={draft.update} boxed unframed />')
    expect(targets).toContain('trackers={trackers} flat')
  })

  it('names the block once, without a second grey line under it', () => {
    // "Posted on the ticket exactly as written here" sat under the title as a
    // third grey size stacked above the prose, which is what made this corner
    // read as blurred. The framed, titled card answers what it is on its own.
    expect(targets).toContain('THE UPDATE')
    expect(targets).not.toContain('Posted on the ticket exactly as written here.')
  })

  it('sets the update as body copy, not as metadata', () => {
    // The `Field`/`Bullets` kit is tuned for labels beside a card - an 11px
    // uppercase kicker in `--t-faint` over 12px `--t-muted` lines. This is the
    // text that gets posted on someone's ticket, and at those sizes and
    // contrasts a paragraph of real prose reads as blurred rather than quiet.
    const body = targets.slice(targets.indexOf('export function UpdateBody'), targets.indexOf('export function updateToText'))
    expect(body).toContain("color: 'var(--t-title)', fontSize: 13, lineHeight: 1.6")
    // Only the compact in-row variant still uses the kit.
    expect(body).toContain('sections.map((sec, i) => boxed ? (')
  })

  it('does not frame it inside a target row, which is already the frame', () => {
    // Per-ticket bodies render inside the row's own tinted surface; a second
    // card in there nests one box inside another.
    const row = targets.slice(targets.indexOf('function TargetRow'))
    expect(row).toContain('{body && <UpdateBody update={body} />}')
    expect(row).not.toContain('boxed')
  })

  it('renders the status as a verdict, not another bulleted section', () => {
    // It used to be a third `Field` identical to Decisions and Verification, so
    // "Shipped" read as one more heading with one more thing under it.
    const body = targets.slice(targets.indexOf('export function UpdateBody'))
    expect(body).toContain('>STATUS</span>')
    expect(body).not.toContain("<Field label=\"Status\">")
  })
})

describe('the update can leave the app', () => {
  it('flattens to text that survives being pasted anywhere', async () => {
    const { updateToText } = await import('../components/timeline/WorklogTargets')
    const text = updateToText({
      summary: 'Feed loads instantly now.',
      sections: [
        { heading: 'Decisions', points: ['Dropped offset pagination.', 'Keyed on (created_at, id).'] },
        { heading: 'Empty', points: ['  '] },
        { heading: '', points: ['orphan'] },
      ],
      status: 'Shipped',
    })
    expect(text).toBe(
      'Feed loads instantly now.\n\n'
      + '## Decisions\n- Dropped offset pagination.\n- Keyed on (created_at, id).\n\n'
      + 'Status: Shipped',
    )
  })

  it('offers the copy on the update itself', () => {
    // The draft is often wanted where the tracker is not - a standup, a PR body.
    // Hand-selecting prose that spans headings picks the labels up as body text.
    expect(targets).toContain('navigator.clipboard.writeText(updateToText(update))')
    expect(targets).toContain('<CopyUpdate update={update} />')
  })
})

describe('the action row is one decision, not three options', () => {
  it('keeps only the two controls that answer "where does this go"', () => {
    // Three identically-bordered buttons read as three options of the same kind.
    // Two of these answer where the update lands; Regenerate answers whether the
    // TEXT is right, which is a question about the document.
    // Comments stripped: the fix left one explaining the bug, and it names the
    // control it removed.
    const row = actions.slice(actions.indexOf('export function DraftActions'),
      actions.indexOf('export function RegenerateDraft'))
      .split('\n').filter((l) => !/^\s*(\/\/|\*|\/\*)/.test(l)).join('\n')
    expect(row).toContain('data-tour="wl-rematch"')
    expect(row).toContain('data-tour="wl-approve"')
    expect(row).not.toContain('Regenerate')
    expect(row).not.toContain('onRegenerate')
  })

  it('moves rewriting onto the document, beside when it was written', () => {
    const dialog = src('components/timeline/WorklogDraftDialog.tsx')
    expect(actions).toContain('export function RegenerateDraft')
    expect(dialog).toContain('<RegenerateDraft busy={busy} onRegenerate={generate} />')
    // Only while there is a draft to rewrite and it has not gone out yet - a
    // posted update is followed up, not overwritten (that is `PostedBar`'s job).
    expect(dialog).toContain('{draft && !posted && <RegenerateDraft')
  })
})
