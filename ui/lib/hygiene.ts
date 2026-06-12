//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Maps the triage reason codes stored in pm_task_curation.reasons_json into the
// human hint + the in-app fix the Tasks view renders. Mirrors the Rust engine's
// TriageReason::hint() and TriageReason::fix() — kept in sync by hand.

export interface HygieneFix {
  control: string // date_picker | assign_self | edit_text | edit_checklist | pick_parent | edit_labels | pick_priority | number_input
  field: string // tracker field the write-back targets
  label: string // button text
  ai: boolean // suggested value comes from AI (wired later)
}

export type Severity = 'must_fix' | 'optional'

export interface HygieneIssue {
  code: string
  hint: string
  fix: HygieneFix | null // null ⇒ handled at ticket level (close / snooze) or descriptive
  severity: Severity // must_fix: Meridian needs it (due date / description / title) — the rest is optional
}

// Must-fix = the fields Meridian needs to track a ticket accurately. Everything
// else is good hygiene the dev can address at leisure on the cleanup page.
const MUST_FIX = new Set([
  'missing_description',
  'thin_description',
  'vague_title',
  'missing_due_date',
])

function reasonSeverity(code: string): Severity {
  return MUST_FIX.has(code) ? 'must_fix' : 'optional'
}

export interface Hygiene {
  bucket: string // ready | needs_detail | looks_stale | not_sure
  issues: HygieneIssue[] // fixable defects (excludes purely-active reasons)
  decision: string | null // keep | excluded | snoozed
}

function reasonHint(code: string, d: Record<string, number> | undefined): string {
  switch (code) {
    case 'in_progress': return 'In progress on the board.'
    case 'due_soon': return (d?.in_days ?? 1) <= 0 ? 'Due today.' : `Due in ${d?.in_days} day(s).`
    case 'in_sprint': return 'In the active sprint.'
    case 'start_date_reached': return 'Its start date has passed.'
    case 'missing_description': return 'No description — nothing to match your work against.'
    case 'thin_description': return `Description is only ${d?.chars} characters.`
    case 'vague_title': return 'Title is generic — make it specific.'
    case 'no_context_anchor': return 'Not linked to an epic or parent.'
    case 'missing_due_date': return "No due date — add one so Meridian knows when it's live."
    case 'missing_assignee': return 'No assignee — who owns this?'
    case 'missing_labels': return 'No labels — add one to categorise it.'
    case 'missing_priority': return 'No priority set.'
    case 'missing_estimate': return 'No estimate — add story points.'
    case 'missing_acceptance_criteria': return "No acceptance criteria — define what 'done' means."
    case 'no_activity_since': return `No board activity in ${d?.days} days.`
    case 'not_started': return 'Still in a not-started column.'
    case 'no_due_date': return 'No due date set.'
    case 'overdue_long': return `Overdue by ${d?.by_days} days with no movement.`
    case 'far_future_due': return `Not due for ${d?.in_days} days — planned, not current work.`
    case 'not_in_sprint': return 'Not in any sprint.'
    case 'already_done': return 'Already marked done.'
    case 'no_activity_signal': return "Open, but nothing yet says it's active."
    case 'unreadable_updated_at': return "Couldn't read its last-updated time."
    default: return code
  }
}

function reasonFix(code: string): HygieneFix | null {
  switch (code) {
    case 'missing_description': return { control: 'edit_text', field: 'description', label: 'Add a description', ai: true }
    case 'thin_description': return { control: 'edit_text', field: 'description', label: 'Expand the description', ai: true }
    case 'vague_title': return { control: 'edit_text', field: 'summary', label: 'Make the title specific', ai: true }
    case 'no_context_anchor': return { control: 'pick_parent', field: 'parent', label: 'Link to an epic or parent', ai: false }
    case 'missing_due_date': return { control: 'date_picker', field: 'duedate', label: 'Add a due date', ai: false }
    case 'missing_assignee': return { control: 'assign_self', field: 'assignee', label: 'Assign to me', ai: false }
    case 'missing_labels': return { control: 'edit_labels', field: 'labels', label: 'Add a label', ai: false }
    case 'missing_priority': return { control: 'pick_priority', field: 'priority', label: 'Set priority', ai: false }
    case 'missing_estimate': return { control: 'number_input', field: 'story_points', label: 'Add an estimate', ai: false }
    case 'missing_acceptance_criteria': return { control: 'edit_checklist', field: 'acceptance_criteria', label: 'Add acceptance criteria', ai: true }
    default: return null // stale / active / descriptive reasons aren't per-field fixes
  }
}

interface RawReason { code: string; detail?: Record<string, number> }

/** Parse a reasons_json blob into the fixable hygiene issues (drops active/descriptive). */
export function parseIssues(reasonsJson: string | null): HygieneIssue[] {
  if (!reasonsJson) return []
  let raw: RawReason[]
  try { raw = JSON.parse(reasonsJson) } catch { return [] }
  return raw
    .map(r => ({
      code: r.code,
      hint: reasonHint(r.code, r.detail),
      fix: reasonFix(r.code),
      severity: reasonSeverity(r.code),
    }))
    .filter(i => i.fix !== null)
}

/** True if any issue is must-fix — drives the Tasks-page banner. */
export function hasMustFix(issues: HygieneIssue[]): boolean {
  return issues.some(i => i.severity === 'must_fix')
}
