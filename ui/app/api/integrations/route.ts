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

export async function GET() {
  const envPaths = [
    path.join(repoRoot(), '.env'),
    path.join(os.homedir(), '.meridian', 'app', '.env'),
  ]
  let env: Record<string, string> = {}
  for (const p of envPaths) {
    const parsed = parseEnv(p)
    if (Object.keys(parsed).length > 0) { env = parsed; break }
  }

  const result: IntegrationsResponse = {
    jira: isSet(env, 'JIRA_BASE_URL') && isSet(env, 'JIRA_EMAIL') && isSet(env, 'JIRA_API_TOKEN'),
    linear: isSet(env, 'LINEAR_API_KEY'),
    github: isSet(env, 'GITHUB_TOKEN'),
  }

  return NextResponse.json(result)
}
