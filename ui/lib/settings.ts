// meridian — normalises screenpipe activity into structured app sessions
import fs from 'fs'
import path from 'path'
import os from 'os'

export interface RuntimeSettings {
  // Observability
  log_level: 'DEBUG' | 'INFO' | 'WARNING' | 'ERROR'
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
    if (fs.existsSync(path.join(dir, 'Cargo.toml'))) return dir
    const parent = path.dirname(dir)
    if (parent === dir) break
    dir = parent
  }
  // Fallback: cwd is typically <repo>/ui, so the repo root is its parent.
  return path.basename(process.cwd()) === 'ui' ? path.dirname(process.cwd()) : process.cwd()
}

const SETTINGS_PATH = path.join(repoRoot(), 'settings.json')
// Pre-fix location — read as a fallback so existing settings aren't lost; the
// next write migrates them to SETTINGS_PATH.
const LEGACY_SETTINGS_PATH = path.join(os.homedir(), '.meridian', 'settings.json')

export function readSettings(): RuntimeSettings {
  for (const p of [SETTINGS_PATH, LEGACY_SETTINGS_PATH]) {
    try {
      const raw = fs.readFileSync(p, 'utf-8')
      return { ...SETTINGS_DEFAULTS, ...JSON.parse(raw) }
    } catch {
      // not at this location — try the next
    }
  }
  return { ...SETTINGS_DEFAULTS }
}

export function writeSettings(settings: RuntimeSettings): void {
  fs.mkdirSync(path.dirname(SETTINGS_PATH), { recursive: true })
  fs.writeFileSync(SETTINGS_PATH, JSON.stringify(settings, null, 2), 'utf-8')
}
