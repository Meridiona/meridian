//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import fs from 'fs'
import path from 'path'
import os from 'os'

export interface RuntimeSettings {
  // Observability
  log_level: 'DEBUG' | 'INFO' | 'WARNING' | 'ERROR'
  otlp_enabled: boolean
  otlp_endpoint: string
  oo_email: string
  oo_password: string
  // ETL
  poll_interval_secs: number
  // Classification
  classification_enabled: boolean
  min_classification_duration_s: number
  classification_timeout_s: number
  agent_auto_floor: number
  agent_queue_floor: number
  // LLM
  llm_prefer_local: boolean
  llm_budget_pct: number
  // Jira updater
  jira_update_enabled: boolean
  // Notifications — master switch + per-event-type toggles + quiet hours.
  // Filtering happens once at the delivery layer (the notification API routes),
  // never in the producers, so every event flows into the outbox and only the
  // user's preferences decide whether it surfaces.
  notifications_enabled: boolean
  notify_plan_nudge: boolean
  notify_worklog_ready: boolean
  notify_system_fault: boolean
  quiet_hours_enabled: boolean
  quiet_hours_start: string // 'HH:MM' local time, inclusive
  quiet_hours_end: string   // 'HH:MM' local time, exclusive
}

export const SETTINGS_DEFAULTS: RuntimeSettings = {
  log_level: 'INFO',
  // OpenObserve export is opt-in: off until the user enables it in Settings.
  otlp_enabled: false,
  otlp_endpoint: '',
  oo_email: '',
  oo_password: '',
  poll_interval_secs: 60,
  classification_enabled: true,
  min_classification_duration_s: 10,
  classification_timeout_s: 120,
  agent_auto_floor: 0.65,
  agent_queue_floor: 0.40,
  llm_prefer_local: true,
  llm_budget_pct: 0.5,
  jira_update_enabled: true,
  notifications_enabled: true,
  notify_plan_nudge: true,
  notify_worklog_ready: true,
  notify_system_fault: true,
  quiet_hours_enabled: false,
  quiet_hours_start: '22:00',
  quiet_hours_end: '08:00',
}

// repoRoot finds the source-checkout root (nearest ancestor with Cargo.toml).
// Used only for the legacy read fallback below — the UI writes to the canonical
// ~/.meridian/settings.json, not here.
function repoRoot(): string {
  let dir = process.cwd()
  for (let i = 0; i < 6; i++) {
    if (fs.existsSync(path.join(/*turbopackIgnore: true*/ dir, 'Cargo.toml'))) return dir
    const parent = path.dirname(/*turbopackIgnore: true*/ dir)
    if (parent === dir) break
    dir = parent
  }
  // Fallback: cwd is typically <repo>/ui, so the repo root is its parent.
  return path.basename(process.cwd()) === 'ui' ? path.dirname(/*turbopackIgnore: true*/ process.cwd()) : process.cwd()
}

// Lazy getters to avoid tracing filesystem ops at build time (Turbopack NFT issue).
// These are only called at runtime when API routes execute.
//
// Canonical settings path — MUST match the daemon's resolution in
// src/config.rs::settings_json_path(). The daemon's cwd varies by install type
// (repo root under `cargo run`, ~/.meridian/app for a bundle), so neither side
// resolves settings.json relative to cwd; both use ~/.meridian/settings.json
// (next to meridian.db), overridable via MERIDIAN_SETTINGS_PATH. The repo-local
// settings.json survives only as a read-time migration fallback.
function getSettingsPath(): string {
  const override = process.env.MERIDIAN_SETTINGS_PATH
  if (override && override.trim()) {
    const expanded = override.startsWith('~/')
      ? path.join(/*turbopackIgnore: true*/ os.homedir(), override.slice(2))
      : override
    return expanded
  }
  return path.join(/*turbopackIgnore: true*/ os.homedir(), '.meridian', 'settings.json')
}

// Legacy location: a source checkout may still carry settings.json in the repo
// root. Read-only fallback so existing dev configs migrate on first write.
function getRepoSettingsPath(): string {
  return path.join(/*turbopackIgnore: true*/ repoRoot(), 'settings.json')
}

export function readSettings(): RuntimeSettings {
  for (const p of [getSettingsPath(), getRepoSettingsPath()]) {
    try {
      const raw = fs.readFileSync(/*turbopackIgnore: true*/ p, 'utf-8')
      const parsed = JSON.parse(raw)
      return {
        ...SETTINGS_DEFAULTS,
        ...parsed,
        // Rust serialises Option::None as JSON null; coerce to '' so TS
        // consumers never encounter null on a string-typed field.
        otlp_endpoint: parsed.otlp_endpoint ?? '',
        oo_email:      parsed.oo_email      ?? '',
        oo_password:   parsed.oo_password   ?? '',
      }
    } catch {
      // not at this location — try the next
    }
  }
  return { ...SETTINGS_DEFAULTS }
}

export function writeSettings(settings: RuntimeSettings): void {
  const settingsPath = getSettingsPath()
  fs.mkdirSync(/*turbopackIgnore: true*/ path.dirname(settingsPath), { recursive: true })
  fs.writeFileSync(/*turbopackIgnore: true*/ settingsPath, JSON.stringify(settings, null, 2), 'utf-8')
}
