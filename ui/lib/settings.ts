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

const SETTINGS_PATH = path.join(os.homedir(), '.meridian', 'settings.json')

export function readSettings(): RuntimeSettings {
  try {
    const raw = fs.readFileSync(SETTINGS_PATH, 'utf-8')
    return { ...SETTINGS_DEFAULTS, ...JSON.parse(raw) }
  } catch {
    return { ...SETTINGS_DEFAULTS }
  }
}

export function writeSettings(settings: RuntimeSettings): void {
  fs.mkdirSync(path.dirname(SETTINGS_PATH), { recursive: true })
  fs.writeFileSync(SETTINGS_PATH, JSON.stringify(settings, null, 2), 'utf-8')
}
