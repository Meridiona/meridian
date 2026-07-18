//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import fs from 'fs'
import path from 'path'
import os from 'os'

import type { LlmProviderId } from './llm-providers'

export interface RuntimeSettings {
  // Appearance
  theme: 'lilac' | 'blush' | 'ink'
  // Observability
  log_level: 'DEBUG' | 'INFO' | 'WARNING' | 'ERROR'
  otlp_enabled: boolean
  otlp_endpoint: string
  oo_email: string
  oo_password: string
  // ETL
  poll_interval_secs: number
  agent_auto_floor: number
  agent_queue_floor: number
  // LLM
  // Which AI runs the prose pipeline. Mirrors RuntimeSettings.llm_provider; the wire
  // forms are the ids in lib/llm-providers.ts. Validated on write by update_settings.
  llm_provider: LlmProviderId
  // WHICH custom endpoint, when llm_provider is 'custom' — the registry may hold several,
  // so the kind alone doesn't name one. Ignored for every other provider (a stale id left
  // behind by switching away is inert: Rust's selected_custom_id() reads it only when
  // llm_provider === 'custom'). The registry itself is NOT mirrored here — it is read
  // keyless through list_custom_llm_providers, so the API keys never enter this type.
  llm_provider_custom_id: string | null
  // Optional model override within the chosen provider. null → the provider's default.
  llm_provider_model: string | null
  llm_budget_pct: number
  // Jira updater
  jira_update_enabled: boolean
  // Notifications — master switch + per-event-type toggles + quiet hours.
  // Filtering happens once at the delivery layer (the notification API routes),
  // never in the producers, so every event flows into the outbox and only the
  // user's preferences decide whether it surfaces.
  notifications_enabled: boolean
  notify_plan_nudge: boolean
  // Daily planner auto-open — the tray opens the dashboard on the Plan modal
  // once per local day. A window behaviour, not a toast, so NOT gated by
  // notifications_enabled.
  auto_open_plan: boolean
  notify_worklog_ready: boolean
  notify_system_fault: boolean
  quiet_hours_enabled: boolean
  quiet_hours_start: string // 'HH:MM' local time, inclusive
  quiet_hours_end: string   // 'HH:MM' local time, exclusive
  work_hours_enabled: boolean
  work_hours_start: string  // 'HH:MM' local time, inclusive
  work_hours_end: string    // 'HH:MM' local time, exclusive
  work_days: string         // comma-separated 1–7 (Mon=1 … Sun=7), e.g. '1,2,3,4,5'
  pause_on_streaming_video: boolean
  // Capture ignore lists — apps (exact app name) and websites (domain) Meridian
  // must never capture. Enforced at the capture frame boundary going forward;
  // history is left untouched. Mirrors RuntimeSettings.ignored_apps/ignored_urls
  // in meridian-core/src/settings.rs.
  ignored_apps: string[]
  ignored_urls: string[]
}

export const SETTINGS_DEFAULTS: RuntimeSettings = {
  theme: 'lilac',
  log_level: 'INFO',
  // OpenObserve export is opt-in: off until the user enables it in Settings.
  otlp_enabled: false,
  otlp_endpoint: '',
  oo_email: '',
  oo_password: '',
  poll_interval_secs: 60,
  agent_auto_floor: 0.65,
  agent_queue_floor: 0.40,
  llm_provider: 'claude',
  llm_provider_custom_id: null,
  llm_provider_model: null,
  llm_budget_pct: 0.5,
  jira_update_enabled: true,
  notifications_enabled: true,
  notify_plan_nudge: true,
  auto_open_plan: true,
  notify_worklog_ready: true,
  notify_system_fault: true,
  quiet_hours_enabled: false,
  quiet_hours_start: '22:00',
  quiet_hours_end: '08:00',
  work_hours_enabled: false,
  work_hours_start: '09:00',
  work_hours_end: '18:00',
  work_days: '1,2,3,4,5',
  // On by default — must match RuntimeSettings::default() in meridian-core/src/settings.rs.
  pause_on_streaming_video: true,
  // Nothing ignored by default.
  ignored_apps: [],
  ignored_urls: [],
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
