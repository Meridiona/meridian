//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// POST /api/auth/oauth/start?provider=jira|trello
//
// Spawns `meridian oauth-login <provider>` as a background process.
// The command opens the user's browser to the provider's OAuth page,
// handles the callback on a local port, and writes the token to
// ~/.meridian/oauth/<provider>.json.
//
// Returns immediately with { started: true }. The UI polls
// /api/integrations until the provider shows connected.

import { NextResponse } from 'next/server'
import { spawn } from 'child_process'
import path from 'path'
import os from 'os'
import fs from 'fs'
import { meridianCandidates, selectMeridianBinary } from '@/lib/meridian-bin'

const ALLOWED = new Set(['jira', 'trello'])

export async function POST(request: Request) {
  const { searchParams } = new URL(request.url)
  const provider = searchParams.get('provider') ?? ''
  if (!ALLOWED.has(provider)) {
    return NextResponse.json({ error: `Unknown provider: ${provider}` }, { status: 400 })
  }

  const bin = selectMeridianBinary(meridianCandidates())
  const logDir = process.env.MERIDIAN_LOG_DIR ?? path.join(os.homedir(), '.meridian', 'logs')

  try {
    // Detached so it survives if Next.js process restarts
    const child = spawn(bin, ['oauth-login', provider], {
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env },
    })

    // Capture output to a temp log for debugging
    const outLog = fs.createWriteStream(path.join(logDir, `oauth-${provider}.log`), { flags: 'a' })
    child.stdout?.pipe(outLog)
    child.stderr?.pipe(outLog)
    child.unref()

    return NextResponse.json({ started: true, provider })
  } catch (e) {
    return NextResponse.json({ error: `Could not start OAuth flow: ${e}` }, { status: 500 })
  }
}
