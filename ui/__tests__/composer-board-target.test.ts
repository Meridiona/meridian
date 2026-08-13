//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The task composer's "where does this go" choice, after the N-cards-per-tracker row
// collapsed into ONE board option with a tracker picker inside it.
//
// `boardProviderFor` / `isBoardTarget` are imported and exercised for real (the store
// module has no DOM dependency). The rendering rules that can't be imported without a
// React harness — which this repo does not have, see task-composer.test.ts — are
// pinned by scanning the source, the same convention the sibling tests use.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import {
  boardProviderFor, isBoardTarget, type ComposerState,
} from '../components/plan/useTaskComposer'
import { LOCAL_PROVIDER } from '../lib/api-types'

const composer = readFileSync(join(import.meta.dir, '../components/plan/TaskComposer.tsx'), 'utf8')
const store = readFileSync(join(import.meta.dir, '../components/plan/useTaskComposer.ts'), 'utf8')

/** A composer state with only the fields these predicates read. */
const state = (over: Partial<ComposerState> = {}): ComposerState => ({
  note: '', title: '', description: '', issueType: 'Task',
  target: LOCAL_PROVIDER, boardProvider: null, phase: 'idle',
  titleDrafted: false, descriptionDrafted: false,
  error: null, errorSource: null, providerDown: false, note_after: null, created: null,
  ...over,
})

describe('boardProviderFor', () => {
  it('is null with no tracker connected - there is no board option to offer', () => {
    expect(boardProviderFor(state(), [])).toBe(null)
    expect(boardProviderFor(state({ boardProvider: 'jira' }), [])).toBe(null)
  })

  it('defaults to the first connected tracker before the user has chosen', () => {
    expect(boardProviderFor(state(), ['jira', 'github'])).toBe('jira')
    expect(boardProviderFor(state(), ['github', 'jira'])).toBe('github')
  })

  it('keeps the tracker the user chose, not whichever sorts first', () => {
    expect(boardProviderFor(state({ boardProvider: 'github' }), ['jira', 'github'])).toBe('github')
  })

  it('falls back to the first tracker when the remembered one was disconnected', () => {
    // Settings can disconnect a tracker while the composer sits open. Filing at a
    // provider with no credentials fails at the daemon, after the user committed.
    expect(boardProviderFor(state({ boardProvider: 'github' }), ['jira'])).toBe('jira')
  })

  it('survives a boardProvider that was never a real tracker id', () => {
    expect(boardProviderFor(state({ boardProvider: 'nonsense' }), ['linear'])).toBe('linear')
  })

  it('is stable for a single connected tracker whatever was remembered', () => {
    for (const remembered of [null, 'jira', 'github', 'trello']) {
      expect(boardProviderFor(state({ boardProvider: remembered }), ['linear'])).toBe('linear')
    }
  })
})

describe('isBoardTarget', () => {
  it('is false for the personal default', () => {
    expect(isBoardTarget(state())).toBe(false)
    expect(isBoardTarget(state({ target: LOCAL_PROVIDER }))).toBe(false)
  })

  it('is true for every provider id', () => {
    for (const id of ['jira', 'linear', 'github', 'trello', 'azure_devops']) {
      expect(isBoardTarget(state({ target: id }))).toBe(true)
    }
  })

  it('does not depend on boardProvider - only where the create actually goes', () => {
    // Remembering GitHub while sitting on Personal must still read as personal,
    // or a trip through the board option would silently file the task.
    expect(isBoardTarget(state({ target: LOCAL_PROVIDER, boardProvider: 'github' }))).toBe(false)
  })
})

describe('the board choice is remembered across a trip through Personal', () => {
  it('setTarget records a provider as the board choice and leaves it on local', () => {
    // The branch is the whole mechanism: picking a provider must ALSO write
    // boardProvider, and selecting Personal must NOT clear it.
    expect(store).toContain(
      "patch(target === LOCAL_PROVIDER ? { target } : { target, boardProvider: target })",
    )
  })

  it('starts on Personal, so filing on a shared board is always a deliberate click', () => {
    expect(store).toContain('target: LOCAL_PROVIDER,')
    expect(store).toContain('boardProvider: null,')
  })
})

describe('the composer renders ONE board option, not one per tracker', () => {
  it('no longer maps trackers into a row of "Create in X" cards', () => {
    expect(/trackers\.map\([^)]*=>\s*\(?\s*<TargetOption/.test(composer)).toBe(false)
  })

  it('offers exactly two TargetOptions - personal and board', () => {
    expect(composer.match(/<TargetOption/g)?.length).toBe(2)
  })

  it('names the tracker directly when there is only one, and generalises past that', () => {
    expect(composer).toContain("trackers.length > 1 ? 'Create on your board' : `Create in ${trackers[0].name}`")
  })

  it('hides the tracker picker when there is nothing to pick between', () => {
    expect(composer).toContain('{trackers.length > 1 && (')
  })

  it('shows a brand mark beside each tracker in the picker', () => {
    expect(composer).toContain('<ProviderIcon provider={t.id} size={12} />')
  })

  it('drops the whole Where block when no tracker is connected', () => {
    // boardProvider is null exactly then - solo users are offered no choice at all.
    expect(composer).toContain('{boardProvider && (')
  })

  it('keeps the tracker picker out of the radio button element', () => {
    // A <button> inside a <button> is invalid HTML and browsers drop one of them,
    // which would make either the card or the chips unclickable.
    expect(/<button[^>]*role="radio"[^>]*>[\s\S]{0,400}?<button/.test(
      composer.slice(composer.indexOf('function TargetOption')),
    )).toBe(false)
  })

  it('repairs a target stranded by a disconnect instead of filing blind', () => {
    expect(composer).toContain('boardProviderFor(s, trackers.map(t => t.id)) ?? LOCAL_PROVIDER')
  })
})

describe('the create still sends one target field', () => {
  it('posts the provider id (or local), never a "board" sentinel', () => {
    // The daemon routes on this exact value; a sentinel would need translating
    // somewhere, and that somewhere is where it would drift.
    expect(store).toContain('target: state.target,')
    expect(store).not.toContain("target: 'board'")
  })
})
