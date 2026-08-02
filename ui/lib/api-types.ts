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
  // Whether this task is on the board. Computed in Rust by the SAME predicate
  // that builds the worklog matcher's candidate set (meridian-core `board.rs`),
  // so this list and what the model compares your work against are one thing.
  // Filter on this, never on `is_terminal` - that would be a second, drifting
  // definition of the same idea, which is the bug this replaced.
  on_board: boolean
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

// The minimal shape `StatusPicker`/`useTaskStatusChange` need to drive a status
// change — deliberately narrower than `TaskSummary` so `TaskDetail` (the plan's
// task-detail dialog) can be passed straight in too, without an adapter. Both
// `TaskSummary` and `TaskDetail` already carry every one of these fields, so
// either satisfies this structurally.
export interface TaskStatusTarget {
  key: string
  provider: string
  status: string
  is_terminal: boolean
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
  /** PM provider this task's worklog was posted to ('jira', …), or null if none
   *  posted. Drives the "posted to {logo}" badge on the timeline card. */
  posted_provider: string | null
  /** Tracker key the posted worklog landed on, or null. */
  posted_target_key: string | null
  /** Deep link to the posted ticket, or null. */
  posted_browse_url: string | null
}

export interface DayTasksResponse {
  day: string
  tasks: DayTask[]
}

/**
 * How long a workstream must run to count as something you *did*.
 *
 * Mirrors `meridian_core::day_evidence::TASK_MIN_MINUTES`. Below this it is a
 * detour, a glance, or a context switch that happened to earn a title - real, and
 * shown on the timeline, but a list of "what you did today" that includes every
 * five-minute glance is a list the reader sees through instantly.
 *
 * Lives here rather than as a literal in the one component that filters on it, so
 * the two sides move together. The scalars carry the server's own value as
 * `task_min_minutes`; prefer that when it is to hand.
 */
export const TASK_MIN_MINUTES = 30

// ── Generate worklog (`generate_day_task_worklog` / `get_day_task_worklog` /
//    `approve_day_task_worklog`) ───────────────────────────────────────────────
// One centralised, provider-agnostic AI call takes a day-task's whole-story
// summary, matches it against the connected tracker's non-terminal tasks (best
// fit or none), and drafts a high-level status update. `match` XOR `propose` is
// set. On approve the draft is posted as a plain status comment (a proposed task
// is created first) and the day-task is linked to the resulting ticket.

/** One existing ticket the update will be posted to. A draft carries 0..N of these
 *  - a strand of a day's work often advances several planned tasks, and the same
 *  update goes on each. Each tracks its own delivery, because posting to three
 *  tickets can succeed on two and a comment cannot be un-posted. */
export interface WorklogTarget {
  task_key: string
  provider: string
  confidence: number       // 0..1, the model's own fit confidence
  /** The user picked this ticket themselves, overriding the model. `confidence` is
   *  then meaningless - render it as their choice, NEVER as a percentage. */
  manual: boolean
  /** Hydrated from the tracker's task title at read time; null if unresolved. */
  task_title: string | null
  /** The comment is live on the tracker. Terminal - it can't be dismissed. */
  posted: boolean
  posted_comment_id: string | null
  browse_url: string | null
  /** A post was started and its outcome never recorded (a crash mid-request). The
   *  comment may or may not be live and nothing can tell - so it is never
   *  auto-retried, and the user has to open the ticket and look. */
  outcome_unknown: boolean
  /** Why this ticket failed, if it did. Its siblings may have succeeded. */
  error: string | null
  /** This ticket's OWN update - the slice of the work that advanced it. When one
   *  day-task advances two tickets, each gets its own body, so they never receive
   *  the same comment. `null`/absent falls back to the draft-level `update` (the
   *  propose branch, a manual retarget, pre-070 rows). */
  update?: GeneratedWorklogUpdate | null
}

// A brand-new task to create when no existing task fits (created on approve).
export interface GeneratedWorklogPropose {
  issue_type: string       // e.g. "Task" | "Bug"
  title: string
  description: string
}

// The high-level status update itself — decisions/architecture/status, NOT a
// time worklog. `summary` is the one-paragraph lead; the arrays add detail.
/** One labelled bullet group in an update. `heading` is model-chosen to fit the
 *  work (dev "Decisions"/"Architecture", marketer "Campaigns", editor "Edits"). */
export interface WorklogSection {
  heading: string
  points: string[]
}

export interface GeneratedWorklogUpdate {
  summary: string
  /** Dynamic, work-fitting labelled bullet groups (0..N; may be empty). */
  sections: WorklogSection[]
  status: string
}

export interface DayTaskWorklogDraft {
  /** `posted` means EVERY target took the update; a partial delivery stays
   *  `approved` and is retryable. */
  state: 'drafted' | 'approved' | 'posted'
  /** The tracker a proposal would be created on. Targets carry their own. */
  provider: string
  /** The tickets this update posts to, strongest match first. Empty when the draft
   *  is a proposal, or once every match has been dismissed. */
  targets: WorklogTarget[]
  propose: GeneratedWorklogPropose | null
  update: GeneratedWorklogUpdate
  reasoning: string
  created_task_key: string | null
  /** The last draft-level failure. Per-ticket failures live on the target. */
  error: string | null
  /** When this draft was last written (generated OR regenerated) — RFC-3339, UTC.
   *  "As of", not "first generated at" — a Regenerate click bumps it. */
  updated_at: string
}

/** The tray's escalate-command reply (`escalate_personal_task_create` /
 *  `escalate_personal_task_match`): the real ticket a personal task graduated to,
 *  plus a browse URL when one can be formed. `created` is true when a brand-new
 *  ticket was filed (vs matched onto an existing one). */
export interface EscalateResponse {
  linked_ticket: string
  provider: string
  browse_url: string | null
  created: boolean
}

/** One ticket the worklog picker can retarget a draft at (tray
 *  `get_board_tickets`). The open board plus personal tasks (`provider ===
 *  'local'`, filed onto their own row rather than posted) - unlike the matcher's
 *  candidates, which are only the day's planned tasks. */
export interface BoardTicket {
  task_key: string
  provider: string
  title: string
  issue_type: string
  epic_title: string
}

/** One ticket's outcome in an approve. */
export interface PostedTarget {
  task_key: string
  posted: boolean
  browse_url: string | null
  error: string | null
}

export interface ApproveWorklogResponse {
  /** Every ticket took the update. A partial success is `false` and retryable. */
  posted: boolean
  targets: PostedTarget[]
  created_task_key: string | null
  created: boolean
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
  // The worklog update Meridian auto-logged onto this personal task (provider
  // 'local') the last time its day-work matched here - null when nothing has
  // been logged, or for a real tracker ticket (whose updates are comments on the
  // tracker, not on the row). Shown in the task dialog so the user can read the
  // auto-posted update and decide whether to escalate it onto a real tracker.
  local_worklog_text: string | null
  local_worklog_posted_at: string | null
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
  // true once JIRA_PROJECT_KEYS is set — jira alone means either auth mode is
  // live; sync additionally needs at least one selected project.
  jira_projects_selected: boolean
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

// ── User-authored tasks (`draft_plan_task` / `create_plan_task` / `edit_plan_task`) ──

/** The sentinel provider for a personal task - one the user wrote that lives only in
 *  Meridian and was never filed on a tracker. Mirrors Rust's
 *  `meridian_core::task_create::LOCAL_PROVIDER`; compare against it rather than
 *  hardcoding the string, since it is what decides "show a key chip / an Open link". */
export const LOCAL_PROVIDER = 'local'

/** The most tasks a day's plan may hold. Mirrors Rust's
 *  `meridian_core::plan::MAX_PLAN_TASKS`, which is the guard that actually holds -
 *  the UI stop is only the faster of the two, so a mismatch here degrades to a
 *  server error rather than an overflowing plan.
 *
 *  It matters beyond tidiness: the plan IS the worklog matcher's candidate set, so
 *  an eleventh task doesn't just clutter a list, it dilutes every match made
 *  against it. */
export const MAX_PLAN_TASKS = 20

/** An AI-drafted task, for the user to review and edit before creating.
 *  Every field may be empty: `error` set + empty fields is the honest answer when the
 *  model was unreachable, and the composer then shows plain editable fields. It is
 *  never a reason to block creation. */
export interface PlanTaskDraft {
  title: string
  description: string
  issue_type: string        // 'Task' | 'Bug'
  error: string | null      // soft: "couldn't draft - write it yourself"
}

export interface CreatePlanTaskBody {
  title: string
  description: string
  issue_type: string
  /** `'local'` for a personal task, or a provider id to file a real ticket. */
  target: string
  /** The day to add it to; empty = today (resolved server-side). */
  day: string
}

export interface CreatedTask {
  task_key: string
  provider: string
  /** True when a real ticket was filed on a tracker. */
  synced: boolean
  /** A soft caveat to surface (e.g. filed, but not on your board until assigned).
   *  Not an error - the task exists and works. */
  note: string | null
}

export interface EditPlanTaskBody {
  task_key: string
  /** `null`/absent leaves the field alone. */
  title?: string
  description?: string
}

export interface EditPlanTaskResult {
  task_key: string
  provider: string
  /** `applied` - it landed; `redirected` - this tracker has no API for it, so offer
   *  `browse_url` instead. */
  status: string
  browse_url: string | null
  reason: string | null
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
  // This machine's pseudonym, identical to the `host.name` value its error
  // telemetry carries in the central backend. Surfaced in Settings → Account so
  // a user can quote it to support; the hash is one-way, so without the user
  // supplying it their error rows cannot be located.
  supportId: string
  // Whether `supportId` above is CURRENTLY the alpha-testing per-user
  // pseudonym rather than the per-machine one — false while signed out, and
  // false again automatically once the alpha window ends. Drives which
  // Support ID description AccountSection.tsx shows, so that copy can't
  // outlive what the pseudonym is actually doing.
  supportIdIsAccountScoped: boolean
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

// ── DMG auto-update (`check_update` / `install_update` / `update-progress`) ─────
// Mirrors `tray/src-tauri/src/update.rs`'s `UpdateStatus` (serde camelCase). A
// failed check is data (`state: 'error'` + `error`), never a thrown command, so
// the banner renders a diagnostic instead of swallowing it.

export interface UpdateStatus {
  state: 'available' | 'uptodate' | 'unsupported' | 'error'
  currentVersion: string
  /** The newer version, when `state === 'available'`. */
  version: string | null
  notes: string | null
  minimumVersion: string | null
  /** Running version is below the manifest floor — installs without a click. */
  mandatory: boolean
  error: string | null
}

// `update-progress` event payload emitted by `download_and_apply`. `contentLength`
// is null when the server didn't send a Content-Length header.
export interface UpdateProgress {
  downloaded: number
  contentLength: number | null
}

// ── What's New (`get_whats_new`) ───────────────────────────────────────────────

export interface ReleaseNote {
  version: string
  date: string
  highlights: string[]
  fixes: string[]
}

export interface RoadmapItem {
  title: string
  status: 'in-progress' | 'planned' | 'considering'
  description: string
}

export interface WhatsNewData {
  releases: ReleaseNote[]
  roadmap: RoadmapItem[]
}

// ── Daily summary (`get_day_summary` / `generate_day_summary`) ────────────────

/** One observation about the day.
 *
 *  Deliberately has NO category, kind, or severity. An earlier shape did
 *  (`achieved` / `overperformed` / `drifted`) and every label turned an
 *  observation into a verdict on the person, which is what made this screen feel
 *  like a scorecard. */
export interface DaySummaryInsight {
  /** The card's heading, in the model's own words. FREE TEXT, never from a fixed
   *  set — a closed vocabulary would make every day fill the same slots whether or
   *  not it had anything to put in them. May be empty on a legacy row. */
  title: string
  text: string
}

/** How a planned ticket actually went. */
export type PlanOutcome = 'done' | 'partial' | 'not_touched'

/** One planned ticket and what became of it. One entry per committed ticket,
 *  always — a ticket the model never mentioned is `not_touched`, so the ledger is
 *  exactly as long as the plan was. */
export interface PlanVerdict {
  task_key: string
  title: string
  outcome: PlanOutcome
  /** One short line saying why. A fact when `certain`, the model's evidence
   *  otherwise, and empty when nothing could be said at all. */
  evidence: string
  /** Measured minutes attributable to it; 0 when no workstream could be tied. */
  minutes: number
  /** The day-task ids this outcome was read off. The work list uses them to mark a
   *  row as on-plan, so the join is the ledger's, not a title match. */
  day_task_ids: string[]
  /** The outcome came from the DATABASE (a posted worklog, a linked ticket, a
   *  closed ticket), not from the model, which cannot overturn it. */
  certain: boolean
  provider: string
  url: string
}

/** The day's plan arithmetic. Every field is computed server-side from `plan`, so
 *  the ring and the ledger beside it are two views of one array. */
export interface Adherence {
  planned: number
  done: number
  partial: number
  not_touched: number
  /** `round(100 * (done + partial/2) / planned)`; 0 when nothing was planned. */
  achievement_pct: number
  /** Minutes of substantial work no planned ticket accounts for. Not a reproach. */
  unplanned_minutes: number
}

/** A day's composed review. `null` from `get_day_summary` until one is generated.
 *
 *  The plan side (`plan`, `adherence`) is resolved deterministically in Rust from
 *  the worklog matches - never from the model - so it holds even on the fallback
 *  path. The model only writes `headline` and `insights`. */
export interface DaySummary {
  day: string
  /** A short warm line above everything. Empty on the fallback path. */
  headline: string
  /** The three insight cards. Empty on the fallback path. */
  insights: DaySummaryInsight[]
  /** One verdict per planned ticket; empty when the day had no plan. */
  plan: PlanVerdict[]
  adherence: Adherence
  /** Who ACTUALLY answered — the resolver degrades to local on failure. */
  provider: string
  /** The model override in force (`sonnet`/`haiku`), or '' for the default. */
  model: string
  /**
   * The summary could not be composed at all (the call failed, or the answer was
   * unparseable). Not an error — `plan` and `adherence` are computed from the
   * database and still hold — but it is why the prose is empty when it is.
   */
  fallback: boolean
  generated_at: string
  /** The newest activity this was composed from. Compared against the live day to
   *  decide staleness; '' on rows written before migration 068. */
  evidence_at: string
}

/** The day's headline numbers, as `day_evidence` derives them. */
export interface DaySummaryScalars {
  /** Engaged seconds — the SAME number the home page's FOCUS card shows. */
  focus_s: number
  /** Coding seconds incl. folded-in agent time — the home page's CODING card. */
  coding_s: number
  /** Workstreams of at least `task_min_minutes`. The count worth saying out loud. */
  task_count: number
  /** The threshold below which a workstream is a detour, not a thing you did. */
  task_min_minutes: number
  workstream_count_including_brief: number
  idle_s: number
  agent_s: number
  session_count: number
  switch_count: number
  /** The day had a committed plan (confirmed, not skipped, not empty). THE branch
   *  between the two versions of the summary screen. */
  planned: boolean
  /** How many tickets that plan held; 0 when `planned` is false. */
  planned_count: number
}

/** What `get_day_summary_data` returns: the day's live figures and whether the
 *  stored summary has fallen behind them. */
export interface DaySummaryData {
  /** name → rows: the day's aggregate shape (`segments`, `apps`, `categories`,
   *  `hours`, `workstreams`). */
  datasets: Record<string, Record<string, unknown>[]>
  scalars: DaySummaryScalars
  /** Newest tracked activity in the day right now, RFC3339. */
  evidence_at: string
  /** The day has moved on far enough since the stored summary was composed to be
   *  worth recomposing. Decided in Rust (`day_summaries::is_stale`) because "far
   *  enough" is a product rule, not a rendering detail. False when there is no
   *  stored summary — there is nothing to recompose. */
  stale: boolean
}

// ── Recent captured apps (`get_recent_capture_apps`) ─────────────────────────

/** One app Meridian has captured recently — the Settings ignore-picker source.
 *  Mirrors `meridian_core::capture_apps::CaptureApp`. */
export interface CaptureApp {
  /** App name exactly as captured — what `ignored_apps` matches against. */
  app: string
  /** Frame count over the window; a recency/volume hint, not a duration. */
  frames: number
}

// ── LLM providers (`detect_llm_providers`, `install_llm_provider`, … ) ───────
//
// The Rust-mirrored invoke contracts for the provider picker. They lived beside
// their consumer in LlmProviderPicker.tsx; centralising them is what stops the
// TS side drifting when the structs in src/llm/detect.rs change.

/** What one real connectivity test found (mirrors `ProviderTestOutcome` in src/llm/detect.rs). */
export type ProviderTestOutcome =
  | { status: 'ok' }
  | { status: 'rate_limited'; message: string }
  | { status: 'failed'; message: string }

/** One recorded test run (mirrors `ProviderTestResult` in src/llm/detect.rs). */
export interface ProviderTestResult {
  id: string
  outcome: ProviderTestOutcome
  elapsed_ms: number
  /** RFC3339 — when this test ran. */
  tested_at: string
}

/** One provider's live install state (mirrors `ProviderStatus` in src/llm/detect.rs). */
export interface ProviderStatus {
  id: string
  installed: boolean
  path: string | null
  /** Always null — Meridian reports *installed*, not *signed in*. See src/llm/detect.rs. */
  authenticated: boolean | null
  /** The last real connectivity test on record, if any. `null` means never tested — not failed. */
  last_test: ProviderTestResult | null
}

// ── install / sign-in outcome ───────────────────────────────────────────────

/** What running a provider's installer or sign-in produced.
 *  Mirrors `InstallOutcome` in `src/llm/detect.rs`.
 *
 *  Lives here rather than beside its consumer because it is an invoke response
 *  contract mirrored from Rust: keeping those in one place is what stops the
 *  two sides drifting silently when the Rust struct changes. */
export interface InstallOutcome {
  /** Whether the install/sign-in itself succeeded. */
  ok: boolean
  /** Human-readable result - shown directly in the picker. */
  message: string
  /** Resolved CLI path once installed, or null if it could not be located. */
  path: string | null
  /** The command that was actually run, for display and debugging. */
  command: string
}

/** `preview_repair` - what a database repair would face, for the confirmation copy. */
export interface RepairPreview {
  damaged: boolean
  /** Tables that cannot be read end to end. */
  corrupt_tables: string[]
  /** Of those, the ones holding user data rather than capture scratch. */
  product_tables: string[]
}
