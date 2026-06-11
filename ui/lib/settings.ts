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
}

export const SETTINGS_DEFAULTS: RuntimeSettings = {
  log_level: 'INFO',
  otlp_enabled: true,
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
}

// The daemon reads the repo-local settings.json (cwd = repo root). The UI must
// write the SAME file or its toggles never reach the daemon. The UI runs with
// cwd = <repo>/ui (launchd WorkingDirectory and `next dev`/`start`), so the repo
// root is the nearest ancestor containing Cargo.toml.
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
function getSettingsPath(): string {
  return path.join(/*turbopackIgnore: true*/ repoRoot(), 'settings.json')
}

function getLegacySettingsPath(): string {
  return path.join(/*turbopackIgnore: true*/ os.homedir(), '.meridian', 'settings.json')
}

export function readSettings(): RuntimeSettings {
  for (const p of [getSettingsPath(), getLegacySettingsPath()]) {
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
