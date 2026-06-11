//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// POST /api/auth/token — write API credentials to .env and reload the daemon.
// Body: { provider: "jira"|"linear"|"github", ...fields }
//
// Jira fields:   { base_url, email, api_token }
// Linear fields: { api_key, team_ids? }
// GitHub fields: { token, project_ids? }
//
// Writes the relevant KEY=value lines to the active .env, then signals
// the daemon to reload so the new credentials take effect immediately.

import { NextResponse } from 'next/server'
import fs from 'fs'
import path from 'path'
import os from 'os'
import net from 'net'

function activeEnvPath(): string {
  return process.env.NODE_ENV === 'production'
    ? path.join(os.homedir(), '.meridian', 'app', '.env')
    : (() => {
        let dir = process.cwd()
        for (let i = 0; i < 6; i++) {
          if (fs.existsSync(path.join(dir, 'Cargo.toml'))) return path.join(dir, '.env')
          const parent = path.dirname(dir)
          if (parent === dir) break
          dir = parent
        }
        return path.join(process.cwd(), '.env')
      })()
}

const PROVIDER_KEYS: Record<string, string[]> = {
  jira:    ['JIRA_BASE_URL', 'JIRA_EMAIL', 'JIRA_API_TOKEN'],
  linear:  ['LINEAR_API_KEY', 'LINEAR_TEAM_IDS'],
  github:  ['GITHUB_TOKEN', 'GITHUB_PROJECT_IDS'],
}

const FIELD_MAP: Record<string, Record<string, string>> = {
  jira:   { base_url: 'JIRA_BASE_URL', email: 'JIRA_EMAIL', api_token: 'JIRA_API_TOKEN' },
  linear: { api_key: 'LINEAR_API_KEY', team_ids: 'LINEAR_TEAM_IDS' },
  github: { token: 'GITHUB_TOKEN', project_ids: 'GITHUB_PROJECT_IDS' },
}

function upsertEnv(envPath: string, updates: Record<string, string>): void {
  const existing = fs.existsSync(envPath) ? fs.readFileSync(envPath, 'utf-8') : ''
  const lines = existing.split('\n')
  const keysToUpdate = new Set(Object.keys(updates))

  // Replace existing lines
  const updated = lines.map(line => {
    const key = line.split('=')[0].trim()
    if (keysToUpdate.has(key)) {
      keysToUpdate.delete(key)
      return `${key}=${updates[key]}`
    }
    return line
  })

  // Append any keys not already present
  for (const key of keysToUpdate) {
    if (updates[key]) updated.push(`${key}=${updates[key]}`)
  }

  fs.writeFileSync(envPath, updated.join('\n'), 'utf-8')
}

function reloadDaemon(): Promise<void> {
  return new Promise((resolve) => {
    const sockPath = path.join(os.homedir(), '.meridian', 'daemon.sock')
    const socket = net.connect(sockPath)
    socket.on('connect', () => { socket.destroy(); resolve() })
    socket.on('error', () => resolve()) // daemon not running — no-op
    setTimeout(() => { socket.destroy(); resolve() }, 500)
  })
}

export async function POST(request: Request) {
  const body = await request.json().catch(() => null)
  if (!body || typeof body.provider !== 'string') {
    return NextResponse.json({ error: 'Missing provider' }, { status: 400 })
  }

  const { provider, ...fields } = body as Record<string, string>
  const fieldMap = FIELD_MAP[provider]
  if (!fieldMap) {
    return NextResponse.json({ error: `Unknown provider: ${provider}` }, { status: 400 })
  }

  // Build env var updates from submitted fields
  const updates: Record<string, string> = {}
  for (const [fieldName, envKey] of Object.entries(fieldMap)) {
    const v = fields[fieldName]?.trim().replace(/[\r\n]/g, '')
    if (v) updates[envKey] = v
  }

  if (Object.keys(updates).length === 0) {
    return NextResponse.json({ error: 'No fields provided' }, { status: 400 })
  }

  // Validate required fields per provider
  const required: Record<string, string[]> = {
    jira:   ['JIRA_BASE_URL', 'JIRA_EMAIL', 'JIRA_API_TOKEN'],
    linear: ['LINEAR_API_KEY'],
    github: ['GITHUB_TOKEN'],
  }
  const missing = (required[provider] ?? []).filter(k => !updates[k])
  if (missing.length > 0) {
    return NextResponse.json({ error: `Missing: ${missing.join(', ')}` }, { status: 400 })
  }

  // Remove old OAuth token for Jira (API token should take priority, but cleaner to remove)
  if (provider === 'jira') {
    const oauthPath = path.join(os.homedir(), '.meridian', 'oauth', 'jira.json')
    try { fs.unlinkSync(oauthPath) } catch { /* not present */ }
  }

  try {
    upsertEnv(activeEnvPath(), updates)
  } catch (e) {
    return NextResponse.json({ error: `Could not write .env: ${e}` }, { status: 500 })
  }

  // Signal daemon to reload (non-fatal if not running)
  await reloadDaemon()

  return NextResponse.json({ ok: true, updated: Object.keys(updates) })
}
