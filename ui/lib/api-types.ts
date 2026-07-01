//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared dashboard response shapes. These used to live in the `app/api/*/route.ts`
// files, but the export cutover deletes `/api` (the data now comes from Rust
// commands over Tauri `invoke`), so the types moved here — the one place both the
// Rust-backed views and any remaining route stubs import them from. The shapes
// still mirror the Rust command return types byte-for-byte.

import type { Interval } from './intervals'
import type { Hygiene } from './hygiene'

// ── Today (`get_today`) ──────────────────────────────────────────────────────

export interface TaskMeta {
  title: string
  provider: string
  url: string
}

export interface AgentSummary {
  started_at: string
  dur: number
  summary: string
}

export interface TodaySession {
  id: number
  app: string
  started_at: string
  dur: number
  cat: string
  titles: string[]
  explain: string | null
  routing: string | null
  session_type: string | null
  task_key: string | null
  candidates: string[]
  confidence: number
  method: string
  link_method: string | null
  link_confidence: number | null
  summary: string | null
}

export interface TodayActive {
  app: string
  started_at: string
  elapsed_s: number
  cat: string
  titles: string[]
  confidence: number
  explain: string | null
}

export interface TodayGap {
  id: number
  kind: string
  started_at: string
  ended_at: string
  dur: number
}

export interface TodayResponse {
  date: string
  sessions: TodaySession[]
  active: TodayActive | null
  gaps: TodayGap[]
  // ── Presence (mutually exclusive: you were either active or idle) ──────────
  focus_s: number        // ACTIVE presence — union of foreground sessions you were engaged in
  idle_s: number         // away from keyboard (user_idle gaps)
  // ── Agent overlay (a layer ON TOP of presence, never additive to focus) ────
  agent_s: number        // engaged coding-agent time (capped to duration_s, unioned)
  supervised_s: number   // agent time that ran WHILE you were active (AI-assisted) — subset of focus_s
  autonomous_s: number   // agent time that ran while you were away (agent_s − supervised_s)
  // ── Timeline bands ─────────────────────────────────────────────────────────
  presence_segments: Interval[] // merged active blocks (foreground), for the day timeline
  agent_segments: Interval[]    // merged engaged-agent blocks, drawn as an overlay band
  // ── Counts ───────────────────────────────────────────────────────────────
  session_count: number  // foreground sessions only
  switch_count: number   // genuine context switches in the foreground stream
  // ── Per-task totals ────────────────────────────────────────────────────────
  task_totals: Record<string, number>
  task_autonomous_s: Record<string, number>
  engaged_s: number
  task_meta: Record<string, TaskMeta>
  task_agent_summaries: Record<string, AgentSummary[]>
}

// ── Tasks (`get_tasks`) ──────────────────────────────────────────────────────

export interface TaskSummary {
  key: string
  title: string
  description: string
  issue_type: string
  status: string        // verbatim provider status / column name (may be empty)
  is_terminal: boolean  // whether that status means the ticket is done/closed
  provider: string
  url: string
  epic_key: string | null
  epic_title: string | null
  due_date: string | null
  start_date: string | null
  today_s: number
  today_autonomous_s: number  // agent time on the task that ran while you were away
  week_s: number
  session_count: number
  cats: Record<string, number>
  hygiene: Hygiene | null  // board-hygiene flags + fixes (null until triaged)
}

export interface TasksResponse {
  tasks: TaskSummary[]
  unassigned_s: number
}

// ── Worklogs (`get_worklogs`) ────────────────────────────────────────────────

export interface WorklogBullet {
  kind: string
  text: string
}

export interface WorklogItem {
  id: number
  task_key: string
  task_title: string | null
  task_url: string | null
  provider: string
  window_start: string
  window_end?: string | null
  state: string
  confidence: number
  coverage: number
  time_spent_seconds: number
  summary: string
  bullets: WorklogBullet[]
  next_steps: string[]
  risk_flags: string[]
  reasoning: string
  posted_worklog_id: string | null
  last_post_error: string | null
  edited: boolean
  /** True when this entry is a tier-3 PROPOSED new ticket (not a real worklog).
   *  Rendered inline in the timeline with an editable title + body + reasoning
   *  and Approve/Dismiss actions. */
  is_proposed?: boolean
  /** `pm_proposed_tasks.id` when `is_proposed` — the key the proposed-ticket
   *  edit/approve/dismiss commands take. */
  proposed_id?: number | null
  /** The proposed ticket's issue type (`Task` / `Bug`) when `is_proposed` —
   *  shown as a chip on the proposal card. Empty for ordinary worklogs. */
  issue_type?: string
}

export interface WorklogsResponse {
  day: string
  items: WorklogItem[]
  counts: Record<string, number>
}

// ── Hour text (`get_hour_text`) ──────────────────────────────────────────────

export interface HourTextResponse {
  hour: string
  // The human-readable activity REPORT (the /activity_report LLM output) —
  // null until the hour has been processed (or for a non-today day; the reader
  // is today-only). Not the raw distilled input. Not an error state.
  report: string | null
  report_chars: number | null
}

// ── Week (`get_week`) ────────────────────────────────────────────────────────

export interface DaySummary {
  day: string
  date: string
  total_s: number
  cats: Record<string, number>
  isToday: boolean
}

export interface WeekResponse {
  days: DaySummary[]
  total_s: number
}

// ── Plan task detail (`get_task_detail`) ─────────────────────────────────────

export interface TaskDetail {
  key: string
  title: string
  provider: string
  url: string
  status: string
  is_terminal: boolean
  issue_type: string
  epic: string | null
  priority: string | null
  story_points: string | null
  due_date: string | null
  due_days: number | null
  start_date: string | null
  description: string
  acceptance_criteria: string | null
}

// ── Integrations (`get_integrations`) ────────────────────────────────────────

export interface IntegrationsResponse {
  jira: boolean
  linear: boolean
  github: boolean
  trello: boolean
  azure_devops: boolean
  sync_errors: Partial<Record<string, string>>
}

// ── Plan (`get_plan` / `plan_action`) ────────────────────────────────────────

// The per-task display meta shared by plan + available rows (was daily-plan.ts's
// `TaskMeta`; renamed to avoid clashing with the Today `TaskMeta` above).
export interface PlanTaskMeta {
  description: string       // short excerpt of description_text
  epic: string | null       // epic_title, else parent_key
  priority: string | null
  issue_type: string
  story_points: string | null
}

export interface PlanItem extends PlanTaskMeta {
  task_key: string
  position: number
  origin: string
  title: string
  provider: string
  url: string
  status: string
  is_terminal: boolean
  due_date: string | null
  due_days: number | null   // whole days until due (negative = overdue), null if no/unparseable date
}

export interface AvailableTask extends PlanTaskMeta {
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

export interface PlanResponse {
  date: string
  has_table: boolean
  confirmed: boolean
  skipped: boolean
  plan: PlanItem[]
  suggestions: AvailableTask[]
  available: AvailableTask[]
}

// ── Notices (`get_notices`) ──────────────────────────────────────────────────

export interface Notice {
  notice_id: string
  severity: 'error' | 'warning'
  title: string
  detail: string
  remedy: string | null
  raised_at: string
}

// ── Banner notifications (`get_banner_notifications`) ─────────────────────────

export interface BannerNotification {
  id: number
  event_key: string
  severity: 'info' | 'warning' | 'error'
  title: string
  body: string
  deep_link: string | null
  created_at: string
}
