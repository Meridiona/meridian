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
  // Split out of WorklogTargets when that file crossed the 500-line rule: that
  // one answers WHERE the worklog goes, this one WHAT goes there.
  const body = src('components/timeline/WorklogUpdateBody.tsx')

  it('draws no card inside the dialog, which is already a card', () => {
    // This was a card, then two cards, then one card with internal header rows -
    // every version a framed surface drawn inside a dialog that already has a
    // frame, a title bar and a footer. Two chromes nested, the inner one
    // repeating the outer one's job: a header saying "THE UPDATE" directly under
    // a title bar saying "Worklog draft". The dialog IS the document.
    const doc = targets.slice(targets.indexOf('export function DraftDocument'))
    expect(doc).not.toContain("border: '1px solid var(--t-card-border)'")
    expect(doc).not.toContain('THE UPDATE')
    // The destination leads, which is why the ticket description folds after
    // three lines: whatever is at the top sets what the reader thinks the
    // screen is about, and that has to be a destination in a sentence rather
    // than a hundred words of model-written ticket body.
    expect(doc.indexOf('sum-where')).toBeLessThan(doc.indexOf('sum-update'))
    expect(targets).toContain('const DESCRIPTION_PEEK = 190')
  })

  it('lays out both surfaces from the same component', () => {
    // The update and its destination are one object - a ticket and the text
    // that goes on it - and both top-level surfaces render the same component,
    // so the walkthrough teaches the screen that actually ships rather than a
    // tutorial-shaped copy of it that drifts.
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
    expect(body).toContain("color: 'var(--t-title)', fontSize: 13, lineHeight: 1.6")
    // Only the compact in-row variant still uses the kit.
    expect(body).toContain('sections.map((sec, i) => boxed ? (')
  })

  it('gives the card three levels, not one uppercase label three times', () => {
    // THE defect in the screenshot. The same 11px uppercase kicker was the
    // card's destination header, the update's title AND every section heading
    // inside it - three unrelated levels of the hierarchy in identical clothes,
    // so the whole card read as one flat slab with no way in.
    // Chrome is now the quietest thing on the card...
    expect(targets).toContain("<p className=\"mt-label mb-2.5\" style={{ color: 'var(--t-faint-2)' }}>")
    // ...the summary is the loudest...
    expect(body).toContain('fontSize: boxed ? 14 : 13')
    // ...and a section heading is a heading rather than a shout.
    expect(body).toContain('{sentenceCase(sec.heading)}')
    expect(body).not.toContain('<p className="mt-label mb-1.5"')
  })

  it('separates sections with a rule, and lets a heading outrank its bullets', () => {
    // With only vertical space between them, three sections of two bullets read
    // as one seven-item list with stray bold lines in it - the reader has to
    // reconstruct the grouping from spacing, which is the work a document is
    // meant to have already done. And the heading was set SMALLER and LIGHTER
    // than the bullets under it, so it read as a caption trailing off the
    // section above rather than the title of the one below.
    expect(body).toContain("className={i === 0 ? '' : 'mt-4 pt-4 border-t'}")
    expect(body).toContain("color: 'var(--t-title)', fontSize: 13, fontWeight: 700,")
    // A drawn dot: `·` sits on the baseline at a size that vanishes beside 13px
    // prose, so the bullets did not read as a list at all.
    expect(body).toContain('width: 4, height: 4, marginTop: 8')
  })

  it('keeps copy as a ghost affordance on the header, not a second control', () => {
    // An outlined pill at the weight of the title itself made a one-affordance
    // header read as a row of two controls - the louder of which mattered less.
    // And `⧉` (U+29C9) is absent from most UI font stacks, so it fell back to a
    // different size and baseline from the label beside it.
    expect(body).toContain("border: 'none', background: 'transparent',")
    // (The glyph survives in the comment explaining why it went; the render
    // path is what must not carry it.)
    expect(body).not.toContain("<span aria-hidden>{done ? '✓' : '⧉'}</span>")
    expect(body).toContain('function CopyIcon')
    // Pulled back out of the gutter, since a borderless button's padding would
    // otherwise inset the icon past the margin the label sits on.
    expect(src('components/timeline/WorklogDraftDialog.tsx')).toContain('<CopyUpdate update={draft.update} />')
  })

  it('folds a long proposed-ticket description instead of leading with it', () => {
    // The model writes a full ticket body, and at a hundred words it was the
    // tallest block in the dialog - sitting ABOVE the update, pushing the thing
    // being judged below the fold, and restating it in a different register.
    expect(targets).toContain('const DESCRIPTION_PEEK = 190')
    expect(targets).toContain('function TicketDescription')
    expect(targets).toContain('Read the full description')
    // And the title leads, rather than being buried mid-sentence behind "New
    // Task:" boilerplate.
    expect(targets).toContain('fontSize: flat ? 15 : 12, fontWeight: 700')
  })

  it('says where the work landed once, not twice', () => {
    // The card header already says "new ticket, nothing on your plan matched".
    // The provenance line then said the same thing again in two sentences
    // directly beneath it, ending in an instruction to use a button six inches
    // further down that is already named for itself.
    expect(dialog).toContain('if (draft.propose) return null')
    expect(dialog).not.toContain('so Meridian drafted a new one')
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
    expect(body).toContain('>STATUS</span>')
    expect(body).not.toContain("<Field label=\"Status\">")
  })
})

describe('the status line', () => {
  it('only wears the chip when it is chip-sized', async () => {
    // The pill was built for "Shipped" and the model does not always oblige.
    // "Antler application submitted and awaiting decision - other accelerator
    // options still being evaluated" arrived as a full sentence inside a green
    // lozenge, which reads as a badge that has burst.
    const { statusIsChipSized } = await import('../components/timeline/WorklogUpdateBody')
    expect(statusIsChipSized('Shipped')).toBe(true)
    expect(statusIsChipSized('Fix open for review')).toBe(true)
    expect(statusIsChipSized('Antler application submitted and awaiting decision')).toBe(false)
    // A sentence is a sentence however short it is.
    expect(statusIsChipSized('Done.')).toBe(false)
  })

  it('leaves a heading the model already wrote as prose alone', async () => {
    const { sentenceCase } = await import('../components/timeline/WorklogUpdateBody')
    expect(sentenceCase('APPLICATION SUBMITTED')).toBe('Application submitted')
    expect(sentenceCase('Selection process')).toBe('Selection process')
    // Not lowercased into nonsense: mixed case means the model meant it.
    expect(sentenceCase('Work on the API layer')).toBe('Work on the API layer')
  })
})

describe('the update can leave the app', () => {
  it('flattens to text that survives being pasted anywhere', async () => {
    const { updateToText } = await import('../components/timeline/WorklogUpdateBody')
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
    expect(src('components/timeline/WorklogUpdateBody.tsx'))
      .toContain('navigator.clipboard.writeText(updateToText(update))')
    expect(src('components/timeline/WorklogDraftDialog.tsx')).toContain('<CopyUpdate update={draft.update} />')
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

  it('sorts the action row by kind: utilities left, the decision right', () => {
    // Rewrite used to sit in the title bar beside the × - a control that
    // REPLACES the document, one target away from the one that dismisses it.
    // It belongs with the other things you can do TO the draft. Both are quiet
    // text on the left; the only loud thing in the row is the one that commits.
    const dialog = src('components/timeline/WorklogDraftDialog.tsx')
    expect(actions).toContain('export function RegenerateDraft')
    expect(dialog).toContain('<RegenerateDraft busy={busy} onRegenerate={generate} />')
    expect(dialog).toContain('<CopyUpdate update={draft.update} />')
    // Only while there is a draft to act on and it has not gone out yet - a
    // posted update is followed up, not overwritten (that is `PostedBar`'s job).
    expect(dialog).toContain('{draft && !posted && !picking && !confirming')
    // And the title bar is a bar again: one control, and it only closes.
    const titleBar = dialog.slice(dialog.indexOf('Worklog draft'), dialog.indexOf('Body — the document surface'))
    expect(titleBar).not.toContain('RegenerateDraft')
  })
})
