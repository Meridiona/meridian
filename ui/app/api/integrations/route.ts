// meridian — normalises screenpipe activity into structured app sessions
import { NextResponse } from 'next/server'
import fs from 'fs'
import path from 'path'
import os from 'os'

export const dynamic = 'force-dynamic'

export interface IntegrationsResponse {
  jira: boolean
  linear: boolean
  github: boolean
  trello: boolean
  azure_devops: boolean
}

function repoRoot(): string {
  let dir = process.cwd()
  for (let i = 0; i < 6; i++) {
    if (fs.existsSync(path.join(dir, 'Cargo.toml'))) return dir
    const parent = path.dirname(dir)
    if (parent === dir) break
    dir = parent
  }
  return path.basename(process.cwd()) === 'ui' ? path.dirname(process.cwd()) : process.cwd()
}

function parseEnv(filePath: string): Record<string, string> {
  try {
    const lines = fs.readFileSync(filePath, 'utf-8').split('\n')
    const out: Record<string, string> = {}
    for (const line of lines) {
      const trimmed = line.trim()
      if (!trimmed || trimmed.startsWith('#')) continue
      const eq = trimmed.indexOf('=')
      if (eq < 1) continue
      const key = trimmed.slice(0, eq).trim()
      const val = trimmed.slice(eq + 1).trim()
      if (key && val) out[key] = val
    }
    return out
  } catch {
    return {}
  }
}

// A value counts as "set" only if it's present and not a leftover template
// placeholder. The .env.example ships commented sample values like
// `https://your-org.atlassian.net`, `your-api-token-here`, `lin_api_your_key_here`,
// `ghp_your_personal_access_token` — if a user uncomments the block but fills in
// only some fields, the untouched placeholders must NOT read as "connected".
function isSet(env: Record<string, string>, key: string): boolean {
  const v = env[key]
  if (!v) return false
  const lower = v.toLowerCase()
  return !lower.includes('your-') && !lower.includes('_your_') && !lower.includes('-here')
}

// In dev the server runs from the repo; in production it runs from the installed
// bundle at ~/.meridian/app/ui. Always stay in that one location — never cross
// the boundary at runtime. OAuth files in ~/.meridian/oauth/ are runtime state
// shared by the daemon and are always accessed directly.
function activeEnvPath(): string {
  return process.env.NODE_ENV === 'production'
    ? path.join(os.homedir(), '.meridian', 'app', '.env')
    : path.join(repoRoot(), '.env')
}

const OAUTH_PROVIDERS = new Set(['jira', 'trello'])
const TOKEN_KEYS: Record<string, string[]> = {
  github: ['GITHUB_TOKEN', 'GITHUB_PROJECT_IDS'],
  linear: ['LINEAR_API_KEY', 'LINEAR_TEAM_IDS'],
  azure_devops: ['AZURE_DEVOPS_PAT', 'AZURE_DEVOPS_URL', 'AZURE_DEVOPS_ORG', 'AZURE_DEVOPS_PROJECT', 'AZURE_DEVOPS_ORG_URL'],
}
const ALL_PROVIDERS = new Set([...OAUTH_PROVIDERS, ...Object.keys(TOKEN_KEYS)])

export async function DELETE(request: Request) {
  const { searchParams } = new URL(request.url)
  const provider = searchParams.get('provider') ?? ''
  if (!ALL_PROVIDERS.has(provider)) {
    return NextResponse.json({ error: 'Invalid provider' }, { status: 400 })
  }

  if (OAUTH_PROVIDERS.has(provider)) {
    const tokenPath = path.join(os.homedir(), '.meridian', 'oauth', `${provider}.json`)
    try { fs.unlinkSync(tokenPath) } catch { /* not present — no-op */ }
  } else {
    const keys = TOKEN_KEYS[provider]!
    const envPath = activeEnvPath()
    if (fs.existsSync(envPath)) {
      const lines = fs.readFileSync(envPath, 'utf-8').split('\n')
      const filtered = lines.filter(l => !keys.some(k => l.trimStart().startsWith(k + '=')))
      fs.writeFileSync(envPath, filtered.join('\n'), 'utf-8')
    }
  }

  return NextResponse.json({ ok: true })
}

export async function GET() {
  const env = parseEnv(activeEnvPath())

  // Jira connects two ways: browser OAuth (a token store written by
  // `meridian oauth-login jira`) OR the legacy basic-auth env trio. Either counts.
  const jiraOAuth = fs.existsSync(
    path.join(os.homedir(), '.meridian', 'oauth', 'jira.json'),
  )
  const jiraBasic =
    isSet(env, 'JIRA_BASE_URL') && isSet(env, 'JIRA_EMAIL') && isSet(env, 'JIRA_API_TOKEN')

  // Trello connects via browser OAuth (`meridian oauth-login trello`).
  const trelloOAuth = fs.existsSync(
    path.join(os.homedir(), '.meridian', 'oauth', 'trello.json'),
  )

  const result: IntegrationsResponse = {
    jira: jiraOAuth || jiraBasic,
    linear: isSet(env, 'LINEAR_API_KEY'),
    github: isSet(env, 'GITHUB_TOKEN'),
    trello: trelloOAuth,
    azure_devops: isSet(env, 'AZURE_DEVOPS_PAT') && (isSet(env, 'AZURE_DEVOPS_URL') || isSet(env, 'AZURE_DEVOPS_ORG') || isSet(env, 'AZURE_DEVOPS_ORG_URL')),
  }

  return NextResponse.json(result)
}
