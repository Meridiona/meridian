//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Server-side data + scoring for the daily plan ("Today's tasks"). Pure board
// signals — no LLM. Builds three things the /api/plan route serves:
//   • plan        — the dev's committed task_keys for the day (joined w/ pm_tasks)
//   • available   — every non-excluded, open board ticket, scored & sorted so the
//                   most-likely-today tasks float to the top (easy to drag/add)
//   • suggestions — the top few not-yet-committed tasks to pre-fill the morning
//
// Scoring is additive across signals, with DUE DATE weighted prominently (a
// near/overdue ticket should surface even if nothing else flags it). Node runtime
// only (uses better-sqlite3 via lib/db).

import type Database from 'better-sqlite3'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'

// ── Types ────────────────────────────────────────────────────────────────────

// Shared ticket context shown on a card so a dev can recognise / decide.
export interface TaskMeta {
  description: string       // short excerpt of description_text
  epic: string | null       // epic_title, else parent_key
  priority: string | null
  issue_type: string
  story_points: string | null
}

export interface PlanItem extends TaskMeta {
  task_key: string
  position: number
  origin: string
  title: string
  provider: string
  url: string
  status: string
  is_terminal: boolean
  due_date: string | null
  due_days: number | null   // whole days until due (negative = overdue), null if no/again unparseable date
}

export interface AvailableTask extends TaskMeta {
  key: string
  title: string
  provider: string
  url: string
  status: string
  is_terminal: boolean
  due_date: string | null
  due_days: number | null
  started: boolean          // status reads as in-progress
  carryover: boolean        // was in the most recent prior day's plan
  worked_recently: boolean  // appeared in app_sessions in the last few days
  score: number
  origin: string            // primary contributing signal (for storage on add)
  reason: string            // short friendly label for the UI
}

/** Short single-line excerpt of a description for card display. */
function excerpt(s: string | null | undefined, n = 130): string {
  const t = (s ?? '').replace(/\s+/g, ' ').trim()
  return t.length > n ? t.slice(0, n - 1).trimEnd() + '…' : t
}

export interface PlanResponse {
  date: string
  has_table: boolean
  confirmed: boolean
  skipped: boolean
  plan: PlanItem[]
  suggestions: AvailableTask[]
  available: AvailableTask[]
}

// ── Tunables ─────────────────────────────────────────────────────────────────

const RECENT_WORK_DAYS = 3      // "worked recently" lookback
const DUE_SOON_DAYS = 14        // due within this counts as a soon signal
const SUGGESTION_CAP = 5        // how many tasks to pre-fill in the morning

// ── Small helpers ────────────────────────────────────────────────────────────

function tableExists(db: Database.Database, name: string): boolean {
  return !!db.prepare(
    "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
  ).get(name)
}

// Lightweight in-progress heuristic for scoring only (NOT a correctness gate —
// the Rust triage engine owns the authoritative startedness). Word-ish contains.
const STARTED_HINTS = [
  'progress', 'doing', 'wip', 'review', 'qa', 'testing', 'dev',
  'implement', 'active', 'building', 'ongoing', 'started',
]
function looksStarted(status: string): boolean {
  const s = (status || '').toLowerCase()
  return STARTED_HINTS.some(h => s.includes(h))
}

/** Whole days until a due date (date-only). Negative = overdue. null if absent/bad. */
export function dueDaysFrom(due: string | null, now: Date): number | null {
  if (!due) return null
  const d = new Date(due.length <= 10 ? `${due}T00:00:00` : due)
  if (isNaN(d.getTime())) return null
  const a = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  const b = new Date(d.getFullYear(), d.getMonth(), d.getDate())
  return Math.round((b.getTime() - a.getTime()) / 86400000)
}

// ── Scoring ──────────────────────────────────────────────────────────────────

function dueComponent(dueDays: number | null): number {
  if (dueDays === null) return 0
  if (dueDays < 0) return 400        // overdue — strongest due signal
  if (dueDays <= 2) return 350
  if (dueDays <= 7) return 250
  if (dueDays <= DUE_SOON_DAYS) return 120
  if (dueDays <= 30) return 40
  return 0                            // far-future / planned — not today's work
}

function dueReason(dueDays: number | null): string | null {
  if (dueDays === null) return null
  if (dueDays < 0) return `Overdue ${-dueDays}d`
  if (dueDays === 0) return 'Due today'
  if (dueDays === 1) return 'Due tomorrow'
  if (dueDays <= DUE_SOON_DAYS) return `Due in ${dueDays}d`
  return null
}

// ── Loaders ──────────────────────────────────────────────────────────────────

interface MetaRow { confirmed_at: string | null; skipped: number }

// Snapshot of a ticket's board fields, captured onto the daily_plan row at
// write time (see /api/plan POST). Mirrors the pm_tasks columns we display.
interface TaskSnapshot {
  title?: string | null; provider?: string | null; url?: string | null
  status_raw?: string | null; is_terminal?: number
  due_date?: string | null; description_text?: string | null
  epic_title?: string | null; parent_key?: string | null
  priority?: string | null; issue_type?: string | null; story_points?: string | null
}

function parseSnapshot(s: string | null): TaskSnapshot | null {
  if (!s) return null
  try { return JSON.parse(s) as TaskSnapshot } catch { return null }
}

/** Committed plan rows joined with their LIVE pm_tasks state. A planned ticket
 *  that has since gone terminal is pruned from pm_tasks (we keep the active board
 *  clean — see migration 044), so the live join misses. In that case we fall back
 *  to the snapshot captured on the plan row, and treat an off-board planned task
 *  as completed (`is_terminal`) — it left the active board, almost always by being
 *  finished. This keeps a planned-then-done task visible in the day's plan with
 *  its real title / description / epic instead of collapsing to a bare key. */
function loadPlan(db: Database.Database, date: string, now: Date): PlanItem[] {
  if (!tableExists(db, 'daily_plan')) return []
  const hasSnapshot = !!db.prepare(
    "SELECT 1 FROM pragma_table_info('daily_plan') WHERE name='task_snapshot'",
  ).get()
  const rows = db.prepare(`
    SELECT p.task_key, p.position, p.origin,
           ${hasSnapshot ? 'p.task_snapshot' : 'NULL AS task_snapshot'},
           (t.task_key IS NOT NULL) AS on_board,
           t.title, t.provider, t.url,
           COALESCE(t.status_raw,'') AS status_raw,
           COALESCE(t.is_terminal,0) AS is_terminal,
           t.due_date, t.description_text, t.epic_title, t.parent_key,
           t.priority, t.issue_type, t.story_points
    FROM daily_plan p
    LEFT JOIN pm_tasks t ON t.task_key = p.task_key
    WHERE p.plan_date = ?
    ORDER BY p.position ASC, p.task_key ASC
  `).all(date) as Array<{
    task_key: string; position: number; origin: string
    task_snapshot: string | null; on_board: number
    title: string | null; provider: string | null; url: string | null
    status_raw: string; is_terminal: number; due_date: string | null
    description_text: string | null; epic_title: string | null; parent_key: string | null
    priority: string | null; issue_type: string | null; story_points: string | null
  }>
  return rows.map(r => {
    const onBoard = !!r.on_board
    // Live board row wins; otherwise fall back to the captured snapshot.
    const snap = onBoard ? null : parseSnapshot(r.task_snapshot)
    const dueDate = (onBoard ? r.due_date : snap?.due_date) ?? null
    return {
      task_key: r.task_key,
      position: r.position,
      origin: r.origin,
      title: (onBoard ? r.title : snap?.title) ?? r.task_key,
      provider: (onBoard ? r.provider : snap?.provider) ?? 'jira',
      url: (onBoard ? r.url : snap?.url) ?? '',
      status: (onBoard ? r.status_raw : (snap?.status_raw ?? '')) || '',
      // Off the active board ⇒ completed for the day's plan (it was pruned, which
      // for a planned ticket means Done). On board ⇒ trust the live flag.
      is_terminal: onBoard ? !!r.is_terminal : true,
      due_date: dueDate,
      due_days: dueDaysFrom(dueDate, now),
      description: excerpt(onBoard ? r.description_text : (snap?.description_text ?? null)),
      epic: ((onBoard ? r.epic_title : snap?.epic_title)?.trim()
        || (onBoard ? r.parent_key : snap?.parent_key)?.trim() || null) ?? null,
      priority: (onBoard ? r.priority : snap?.priority)?.trim() || null,
      issue_type: (onBoard ? r.issue_type : snap?.issue_type)?.trim() || '',
      story_points: (onBoard ? r.story_points : snap?.story_points)?.trim() || null,
    }
  })
}

function loadMeta(db: Database.Database, date: string): MetaRow {
  if (!tableExists(db, 'daily_plan_meta')) return { confirmed_at: null, skipped: 0 }
  const row = db.prepare(
    'SELECT confirmed_at, skipped FROM daily_plan_meta WHERE plan_date = ?',
  ).get(date) as MetaRow | undefined
  return row ?? { confirmed_at: null, skipped: 0 }
}

/** task_keys committed on the most recent planned day before `date`. */
function carryoverKeys(db: Database.Database, date: string): Set<string> {
  if (!tableExists(db, 'daily_plan')) return new Set()
  const prior = db.prepare(
    'SELECT MAX(plan_date) AS d FROM daily_plan WHERE plan_date < ?',
  ).get(date) as { d: string | null }
  if (!prior?.d) return new Set()
  const rows = db.prepare(
    'SELECT task_key FROM daily_plan WHERE plan_date = ?',
  ).all(prior.d) as Array<{ task_key: string }>
  return new Set(rows.map(r => r.task_key))
}

/** task_key → most recent worked timestamp within the lookback window. */
function recentWorkedKeys(db: Database.Database): Map<string, string> {
  const since = new Date(Date.now() - RECENT_WORK_DAYS * 86400000)
  const { start } = localDayBounds(since.toLocaleDateString('en-CA'))
  const rows = db.prepare(`
    SELECT task_key, MAX(started_at) AS last_at
    FROM app_sessions
    WHERE task_key IS NOT NULL AND task_session_type = 'task' AND started_at >= ?
    GROUP BY task_key
  `).all(start) as Array<{ task_key: string; last_at: string }>
  return new Map(rows.map(r => [r.task_key, r.last_at]))
}

interface BoardRow {
  task_key: string; title: string; provider: string | null; url: string | null
  status_raw: string | null; is_terminal: number; due_date: string | null
  decision: string | null; updated_at: string | null
  description_text: string | null; epic_title: string | null; parent_key: string | null
  priority: string | null; issue_type: string | null; story_points: string | null
}

/** Every candidate board ticket (non-excluded), scored and sorted top-first. */
export function buildAvailable(db: Database.Database, date: string): AvailableTask[] {
  const hasCuration = tableExists(db, 'pm_task_curation')
  const rows = db.prepare(`
    SELECT t.task_key, t.title, t.provider, t.url,
           COALESCE(t.status_raw,'') AS status_raw,
           COALESCE(t.is_terminal,0) AS is_terminal,
           t.due_date, t.updated_at,
           t.description_text, t.epic_title, t.parent_key,
           t.priority, t.issue_type, t.story_points,
           ${hasCuration ? 'c.decision' : 'NULL'} AS decision
    FROM pm_tasks t
    ${hasCuration ? 'LEFT JOIN pm_task_curation c ON c.task_key = t.task_key' : ''}
  `).all() as BoardRow[]

  const carry = carryoverKeys(db, date)
  const worked = recentWorkedKeys(db)
  const now = new Date()
  const nowMs = now.getTime()

  const items: AvailableTask[] = []
  for (const r of rows) {
    if (r.decision === 'excluded') continue          // honour board cleanup
    const isTerminal = !!r.is_terminal
    if (isTerminal) continue                          // done tickets aren't today's work
    const dueDays = dueDaysFrom(r.due_date, now)
    const started = looksStarted(r.status_raw ?? '')
    const carryover = carry.has(r.task_key)
    const workedAt = worked.get(r.task_key) ?? null
    const workedRecently = workedAt !== null

    // recency-of-work component
    let recentComp = 0
    if (workedAt) {
      const ageDays = (nowMs - new Date(workedAt).getTime()) / 86400000
      recentComp = ageDays < 1 ? 200 : ageDays < 2 ? 150 : 80
    }
    // small updated_at tiebreaker (0..30)
    let updComp = 0
    if (r.updated_at) {
      const ageDays = (nowMs - new Date(r.updated_at).getTime()) / 86400000
      if (!isNaN(ageDays)) updComp = Math.max(0, 30 - Math.min(30, Math.floor(ageDays)))
    }

    const score =
      (carryover ? 500 : 0) +
      (started ? 300 : 0) +
      dueComponent(dueDays) +
      recentComp +
      updComp

    // primary origin + friendly reason (highest-weight signal wins)
    let origin = 'manual'
    let reason = 'On your board'
    const dr = dueReason(dueDays)
    if (carryover) { origin = 'carryover'; reason = 'Carried over' }
    else if (started) { origin = 'in_progress'; reason = 'In progress' }
    else if (dr) { origin = 'due_soon'; reason = dr }
    else if (workedRecently) { origin = 'recent'; reason = recentComp >= 150 ? 'Worked recently' : 'Worked this week' }
    // Always surface a due pill reason as secondary even when origin differs —
    // the UI reads due_days directly, so `reason` is just the lead label.

    items.push({
      key: r.task_key,
      title: r.title,
      provider: r.provider || 'jira',
      url: r.url || '',
      status: r.status_raw || '',
      is_terminal: isTerminal,
      due_date: r.due_date ?? null,
      due_days: dueDays,
      started,
      carryover,
      worked_recently: workedRecently,
      score,
      origin,
      reason,
      description: excerpt(r.description_text),
      epic: (r.epic_title?.trim() || r.parent_key?.trim() || null) ?? null,
      priority: r.priority?.trim() || null,
      issue_type: r.issue_type?.trim() || '',
      story_points: r.story_points?.trim() || null,
    })
  }

  // Highest score first; stable tiebreak on key so order is deterministic.
  items.sort((a, b) => (b.score - a.score) || a.key.localeCompare(b.key))
  return items
}

/** Full plan payload for a day. `db` and `available` may be supplied by a write
 *  handler that has already opened the DB and scored the board, so a POST scores
 *  the board once instead of twice. */
export function buildPlanResponse(
  date: string,
  db: Database.Database = getDb(),
  available: AvailableTask[] = buildAvailable(db, date),
): PlanResponse {
  const now = new Date()
  const hasTable = tableExists(db, 'daily_plan')
  const meta = loadMeta(db, date)

  const plan = loadPlan(db, date, now)
  const committed = new Set(plan.map(p => p.task_key))
  const suggestions = available
    .filter(a => !committed.has(a.key) && a.score > 0)
    .slice(0, SUGGESTION_CAP)

  return {
    date,
    has_table: hasTable,
    confirmed: meta.confirmed_at !== null,
    skipped: meta.skipped === 1,
    plan,
    suggestions,
    available,
  }
}

export { todayString }
