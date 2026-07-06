//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import type { WorklogItem } from '../lib/api-types'

// Pure logic extracted from TimelineCard: when to show a tracker link vs a
// plain key chip, and when the "Open in tracker" button is visible.

function shouldShowTrackerLink(item: Pick<WorklogItem, 'task_key' | 'task_url'>): boolean {
  return !!(item.task_key && item.task_url)
}

function trackerLinkText(key: string): string {
  return `${key} ↗`
}

function shouldShowOpenInTrackerButton(item: Pick<WorklogItem, 'task_url'>): boolean {
  return !!item.task_url
}

// Minimal WorklogItem helper — only the fields under test.
const item = (overrides: Partial<WorklogItem> = {}): WorklogItem => ({
  id: 1,
  task_key: 'KAN-290',
  task_title: 'Do the thing',
  task_url: null,
  provider: 'jira',
  window_start: '2026-07-06T09:00:00Z',
  state: 'posted',
  confidence: 0.9,
  coverage: 0.8,
  time_spent_seconds: 1800,
  summary: 'Worked on stuff',
  bullets: [],
  next_steps: [],
  risk_flags: [],
  reasoning: 'It matched',
  posted_worklog_id: null,
  last_post_error: null,
  edited: false,
  ...overrides,
})

// ---------------------------------------------------------------------------
// Key chip: link vs plain span
// ---------------------------------------------------------------------------

describe('TimelineCard: key chip tracker link', () => {
  it('shows a clickable link when both task_key and task_url are set', () => {
    expect(shouldShowTrackerLink(item({ task_url: 'https://myorg.atlassian.net/browse/KAN-290' }))).toBe(true)
  })

  it('shows a plain span when task_url is null', () => {
    expect(shouldShowTrackerLink(item({ task_url: null }))).toBe(false)
  })

  it('shows a plain span when task_key is absent', () => {
    expect(shouldShowTrackerLink({ task_key: '', task_url: 'https://example.com' })).toBe(false)
  })

  it('shows a plain span when both task_key and task_url are absent', () => {
    expect(shouldShowTrackerLink({ task_key: '', task_url: null })).toBe(false)
  })

  it('key chip text appends the ↗ arrow when linked', () => {
    expect(trackerLinkText('KAN-290')).toBe('KAN-290 ↗')
  })

  it('key chip text works for GitHub-style keys', () => {
    expect(trackerLinkText('GH-42')).toBe('GH-42 ↗')
  })

  it('key chip text works for Linear-style keys', () => {
    expect(trackerLinkText('ENG-1')).toBe('ENG-1 ↗')
  })
})

// ---------------------------------------------------------------------------
// "Open in tracker" button visibility
// ---------------------------------------------------------------------------

describe('TimelineCard: Open in tracker button', () => {
  it('shows button when task_url is a valid https URL', () => {
    expect(shouldShowOpenInTrackerButton({ task_url: 'https://myorg.atlassian.net/browse/KAN-290' })).toBe(true)
  })

  it('hides button when task_url is null', () => {
    expect(shouldShowOpenInTrackerButton({ task_url: null })).toBe(false)
  })

  it('hides button when task_url is empty string', () => {
    expect(shouldShowOpenInTrackerButton({ task_url: '' })).toBe(false)
  })

  it('shows button for GitHub issue URLs', () => {
    expect(shouldShowOpenInTrackerButton({ task_url: 'https://github.com/org/repo/issues/42' })).toBe(true)
  })

  it('shows button for Linear issue URLs', () => {
    expect(shouldShowOpenInTrackerButton({ task_url: 'https://linear.app/team/issue/ENG-1' })).toBe(true)
  })

  it('shows button for Azure DevOps URLs', () => {
    expect(shouldShowOpenInTrackerButton({ task_url: 'https://dev.azure.com/org/project/_workitems/edit/42' })).toBe(true)
  })

  it('button is shown regardless of item state (pending, posted, approved, dismissed)', () => {
    for (const state of ['drafted', 'approved', 'posted', 'dismissed', 'skipped']) {
      const i = item({ task_url: 'https://example.com', state })
      expect(shouldShowOpenInTrackerButton(i)).toBe(true)
    }
  })

  it('button is hidden for all states when task_url is null', () => {
    for (const state of ['drafted', 'approved', 'posted', 'dismissed', 'skipped']) {
      const i = item({ task_url: null, state })
      expect(shouldShowOpenInTrackerButton(i)).toBe(false)
    }
  })
})

// ---------------------------------------------------------------------------
// Rust reader: task_url filter (mirrors meridian-core/src/readers/worklogs.rs)
// The Rust code does: task_url.filter(|u| u.starts_with("https://"))
// These tests document the contract so UI and backend stay in sync.
// ---------------------------------------------------------------------------

function rustFilterTaskUrl(raw: string | null | undefined): string | null {
  if (!raw) return null
  return raw.startsWith('https://') ? raw : null
}

describe('task_url filter contract (mirrors Rust reader)', () => {
  it('keeps https:// URLs', () => {
    expect(rustFilterTaskUrl('https://example.atlassian.net/browse/KAN-1')).not.toBeNull()
  })

  it('drops http:// URLs', () => {
    expect(rustFilterTaskUrl('http://example.com/issue/1')).toBeNull()
  })

  it('drops empty string', () => {
    expect(rustFilterTaskUrl('')).toBeNull()
  })

  it('drops null', () => {
    expect(rustFilterTaskUrl(null)).toBeNull()
  })

  it('drops undefined', () => {
    expect(rustFilterTaskUrl(undefined)).toBeNull()
  })

  it('keeps GitHub https URLs', () => {
    expect(rustFilterTaskUrl('https://github.com/org/repo/issues/42')).toBe('https://github.com/org/repo/issues/42')
  })
})
