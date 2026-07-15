//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { TimelineCard } from 'meridian-design-system'

const approved = {
  id: 1, task_key: 'MER-482', task_title: 'Fix ETL gap detection', task_url: 'https://meridiona.atlassian.net/browse/MER-482',
  provider: 'jira', window_start: '2026-07-09T14:00:00Z', window_end: '2026-07-09T15:00:00Z',
  state: 'approved', confidence: 0.94, coverage: 0.8, time_spent_seconds: 2700,
  summary: 'Closed a gap-detection bug where a sleep spanning an ETL run boundary was silently dropped.',
  bullets: [{ kind: 'change', text: 'Added a cross-run gap check before the first batch of a new poll' }],
  next_steps: [], risk_flags: [], reasoning: 'High OCR + window-title overlap with MER-482 across the full hour.',
  posted_worklog_id: null, last_post_error: null, edited: false, issue_type: 'Bug',
}

const proposal = {
  id: 2, task_key: 'MER-NEW-1', task_title: 'Add retry with backoff to hf-proxy fetch', task_url: null,
  provider: 'jira', window_start: '2026-07-09T10:00:00Z', window_end: '2026-07-09T11:00:00Z',
  state: 'proposed', confidence: 0.71, coverage: 0.55, time_spent_seconds: 1800,
  summary: 'Session matched no existing ticket - proposing a new one from the work observed.',
  bullets: [], next_steps: ['Confirm scope with the team'], risk_flags: [], reasoning: 'Keyword match on "hf-proxy" and "retry" across 3 sessions.',
  posted_worklog_id: null, last_post_error: null, edited: false, is_proposed: true, issue_type: 'Task',
}

const rejected = {
  id: 3, task_key: 'MER-410', task_title: 'Update onboarding copy', task_url: 'https://meridiona.atlassian.net/browse/MER-410',
  provider: 'jira', window_start: '2026-07-09T09:00:00Z', window_end: '2026-07-09T09:30:00Z',
  state: 'dismissed', confidence: 0.4, coverage: 0.3, time_spent_seconds: 900,
  summary: 'Low-confidence match, dismissed by reviewer.',
  bullets: [], next_steps: [], risk_flags: [], reasoning: 'Weak signal - window titles mostly idle.',
  posted_worklog_id: null, last_post_error: null, edited: false, issue_type: 'Task',
}

export function CompactApproved() {
  return <div style={{ width: 340 }}><TimelineCard item={approved} /></div>
}

export function CompactProposal() {
  return <div style={{ width: 340 }}><TimelineCard item={proposal} /></div>
}

export function CompactRejected() {
  return <div style={{ width: 340 }}><TimelineCard item={rejected} /></div>
}

export function DetailView() {
  return <div style={{ width: 420 }}><TimelineCard item={approved} variant="detail" /></div>
}

export function SelectedCompact() {
  return <div style={{ width: 340 }}><TimelineCard item={approved} selected /></div>
}
