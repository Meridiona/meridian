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

// ── Coding agents (`get_coding_agents`) ──────────────────────────────────────

export interface AgentTotal {
  app: string     // "Claude Code" / "Codex" / "GitHub Copilot" / "Cursor Agent"
  total_s: number // union seconds across that agent's sessions today (overlap counted once)
}

export interface CodingAgentsResponse {
  date: string
  total_s: number // union across ALL coding-agent sessions (overlap deduped)
  agents: AgentTotal[]
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

// ── Task status change (`list_task_statuses` / `set_task_status`) ─────────────
// The real workflow statuses a task's tracker offers (Jira transitions, Linear
// workflow states, Trello lists, Azure states, GitHub open/closed). `category`
// is the canonical taxonomy (backlog|todo|in_progress|in_review|done|cancelled|
// unknown) so the UI can colour/decide "done" consistently across providers.
export interface TaskStatusOption {
  id: string
  name: string
  category: string
}

export interface StatusListResponse {
  statuses: TaskStatusOption[]
  current_id: string | null
  current_name: string | null
}

export interface SetStatusResponse {
  // Mirrors apply_ticket_fix: 'applied' wrote to the tracker; 'redirected' means
  // the tracker couldn't do it in-app and browse_url was opened instead.
  result: { status: string; browse_url?: string; reason?: string }
  new_status: TaskStatusOption | null
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
  /** The ticket's issue type (`Task` / `Bug` / `Story`, etc), shown as a chip
   *  next to `task_key` on the card. For a real worklog, pulled from the
   *  matched `pm_tasks.issue_type` (empty if the task row is missing or its
   *  type was never fetched from the tracker). For a proposed ticket
   *  (`is_proposed`), the drafted type used when the ticket is created. */
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

// ── Hour reports (`get_hour_reports`) ─────────────────────────────────────────

export interface HourReportEntry {
  hour: number             // local hour, 0..24
  report: string | null    // the /activity_report markdown, or null if not yet available
}

export interface HourReportsResponse {
  day: string
  hours: HourReportEntry[]
}

// ── Day tasks (`get_day_tasks`) ───────────────────────────────────────────────
// Meridian's own inferred day-level tasks (workstreams), folded hour by hour by
// the worklog pipeline. Each task carries approximate time `segments` (multiple =
// breaks) so the timeline can draw it spanning its real start-end; `hours` is the
// coarse per-hour span the interim UI still renders from. `linked_ticket` is the
// PM seam — always null for now.

// One approximate "HH:MM"-"HH:MM" local range a task was worked in. Non-contiguous
// segments on the same task are breaks — the timeline draws them as one workstream.
export interface DaySegment {
  start: string            // local "HH:MM", 24-hour
  end: string              // local "HH:MM", 24-hour ("24:00" = end of day)
}

export interface DayTask {
  id: string               // stable within the day: "T1", "T2", …
  title: string
  summary: string[]        // running log lines (past-tense, one thing done each)
  minutes: number          // deterministic measured minutes (summed segment durations)
  hours: string[]          // local hour labels, "YYYY-MM-DDTHH", ascending
  segments: DaySegment[]   // approximate time ranges worked, ascending; gaps = breaks
  first_hour: number       // earliest local hour-of-day (0..23); -1 if none
  last_hour: number        // latest local hour-of-day (0..23); -1 if none
  status: string
  linked_ticket: string | null
}

export interface DayTasksResponse {
  day: string
  tasks: DayTask[]
}

// ── Generate worklog (`generate_day_task_worklog` / `get_day_task_worklog` /
//    `approve_day_task_worklog`) ───────────────────────────────────────────────
// One centralised, provider-agnostic AI call takes a day-task's whole-story
// summary, matches it against the connected tracker's non-terminal tasks (best
// fit or none), and drafts a high-level status update. `match` XOR `propose` is
// set. On approve the draft is posted as a plain status comment (a proposed task
// is created first) and the day-task is linked to the resulting ticket.

// The chosen existing task the update will comment on (best fit found).
export interface GeneratedWorklogMatch {
  task_key: string
  confidence: number       // 0..1, the model's own fit confidence
}

// A brand-new task to create when no existing task fits (created on approve).
export interface GeneratedWorklogPropose {
  issue_type: string       // e.g. "Task" | "Bug"
  title: string
  description: string
}

// The high-level status update itself — decisions/architecture/status, NOT a
// time worklog. `summary` is the one-paragraph lead; the arrays add detail.
export interface GeneratedWorklogUpdate {
  summary: string
  decisions: string[]
  architecture: string[]
  status: string
}

export interface DayTaskWorklogDraft {
  state: 'drafted' | 'approved' | 'posted'
  provider: string
  match: GeneratedWorklogMatch | null
  propose: GeneratedWorklogPropose | null
  update: GeneratedWorklogUpdate
  reasoning: string
  target_key: string | null       // matched or created key; null until known
  created_task_key: string | null
  posted_comment_id: string | null
  browse_url: string | null
  error: string | null
}

export interface ApproveWorklogResponse {
  posted: boolean
  target_key: string | null
  created_task_key: string | null
  created: boolean
  browse_url: string | null
  error: string | null
}

// ── Hour status (`get_hour_status`) ───────────────────────────────────────────

export interface HourStatus {
  hour: number            // local hour, 0..24
  generating: boolean     // this hour's worklog is being generated right now
  paused: boolean         // tracking was paused at some point during this hour
}

export interface HourStatusResponse {
  day: string
  hours: HourStatus[]
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
  // true once GITHUB_PROJECT_IDS is set — github alone only means the OAuth
  // token exists; sync additionally needs at least one selected project.
  github_projects_selected: boolean
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

// ── Uninstall wizard (`get_uninstall_plan` / `execute_uninstall`) ────────────

export interface UninstallItem {
  label: string
  path: string
}

export interface UninstallPlan {
  agents: UninstallItem[]
  staged_binaries: UninstallItem[]
  data: UninstallItem[]
  runtime: UninstallItem[]
  models: UninstallItem[]
  error?: string
}

export interface UninstallResult {
  removed: string[]
  errors: string[]
  error?: string
}

// ── App info (`get_app_info`) ──────────────────────────────────────────────────

export interface AppInfo {
  version: string
  channel: 'dev' | 'staging' | 'prod'
}

// ── LLM Lab (`get_llm_experiments` / `get_llm_experiment` / `run_llm_experiment`)
// Dev-only multi-provider comparison harness: replay one prose stage from stored
// inputs across several provider/model variants. Mirrors the Rust serde shapes in
// meridian-core/src/readers/llm_experiments.rs and the run body in
// tray/src-tauri/src/commands/llm_lab.rs. Never rendered outside a dev build.

export type LlmExperimentProcess = 'hour_report' | 'workstream_fold' | 'worklog_generate' | 'day_fold'

export interface LlmExperimentSummary {
  id: number
  process: string           // LlmExperimentProcess wire form
  input_ref: string         // "YYYY-MM-DDTHH" or "YYYY-MM-DD/<task_id>"
  status: 'running' | 'done' | 'failed'
  n_variants: number
  n_done: number            // variants already terminal - the progress numerator
  created_at: string
  finished_at: string | null
}

export interface LlmExperimentResult {
  variant_idx: number
  provider: string          // LlmProviderId wire form
  model: string             // '' = the provider's default model
  params_json: string
  status: 'pending' | 'running' | 'ok' | 'failed' | 'rate_limited'
  output_text: string | null      // the model's raw answer
  output_rendered: string | null  // what the pipeline would have made of it
  error: string | null
  input_tokens: number | null     // CLI backends report 0 - render as "-", not 0
  output_tokens: number | null
  elapsed_s: number | null
  started_at: string | null
  finished_at: string | null
}

export interface LlmExperimentDetail extends Omit<LlmExperimentSummary, 'n_done'> {
  input_json: string        // {"user","label","render_ctx"} - the exact request sent
  results: LlmExperimentResult[]
}

export interface RunLlmExperimentBody {
  process: LlmExperimentProcess
  hour?: string             // the two hour processes
  day?: string              // worklog_generate…
  task_id?: string          // …with this
  variants: string[]        // "provider" or "provider:model" tokens
}
