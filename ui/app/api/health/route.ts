// meridian — normalises screenpipe activity into structured app sessions
//
// Health check for the status banner. Uses direct fast checks (fs + launchctl)
// instead of `meridian doctor` — never blocks the event loop, responds in <5ms.

import { access, constants, readFile } from 'fs/promises'
import { exec } from 'child_process'
import { NextResponse } from 'next/server'
import os from 'os'
import path from 'path'

interface HealthStatus {
  a11y_helper_trusted?: boolean
  database_ready?: boolean
  daemon_running?: boolean
  error?: string
}

const CACHE_TTL_MS = 60_000
let cache: { result: HealthStatus; at: number } | null = null
let inFlight: Promise<void> | null = null

function dbPath(): string {
  const fromEnv = process.env.MERIDIAN_DB_PATH
  return fromEnv ?? path.join(os.homedir(), '.meridian', 'meridian.db')
}

function a11yLogPath(): string {
  return path.join(os.homedir(), '.meridian', 'logs', 'a11y-helper.log')
}

async function checkDatabase(): Promise<{ ready: boolean; error?: string }> {
  try {
    await access(dbPath(), constants.R_OK)
    return { ready: true }
  } catch {
    return {
      ready: false,
      error: 'Database not found — start the daemon: launchctl load ~/Library/LaunchAgents/com.meridiona.daemon.plist',
    }
  }
}

// The a11y-helper daemon logs its trust state on every tick. Scan the tail of
// the log for the most recent entry — avoids spawning a subprocess entirely.
async function checkA11yTrusted(): Promise<boolean | undefined> {
  try {
    const raw = await readFile(a11yLogPath(), 'utf8')
    const lines = raw.trimEnd().split('\n')
    // Walk backwards for the latest trust-state line.
    for (let i = lines.length - 1; i >= Math.max(0, lines.length - 200); i--) {
      const l = lines[i]
      if (l.includes('trusted: true') || l.includes('[trusted]')) return true
      if (l.includes('trusted: false') || l.includes('[untrusted]')) return false
    }
    return undefined // log exists but no trust entry yet
  } catch {
    return undefined // log absent → helper not started yet, don't show banner
  }
}

// Fallback: ask launchctl for the a11y-helper service state. Used only when
// the log check is inconclusive (returns undefined).
function launchctlA11yTrusted(): Promise<boolean | undefined> {
  return new Promise((resolve) => {
    const uid = process.getuid?.() ?? 501
    exec(
      `launchctl print gui/${uid}/com.meridiona.a11y-helper`,
      { timeout: 3000 },
      (_err, stdout) => {
        if (!stdout) { resolve(undefined); return }
        if (stdout.includes('a11y_trusted = 1') || stdout.includes('trusted')) {
          resolve(true)
        } else if (stdout.includes('a11y_trusted = 0')) {
          resolve(false)
        } else {
          resolve(undefined)
        }
      },
    )
  })
}

// Check whether the Meridian daemon launchd job is currently running.
// Returns undefined when launchctl is unavailable (non-macOS future).
function checkDaemonRunning(): Promise<boolean | undefined> {
  return new Promise((resolve) => {
    const uid = process.getuid?.() ?? 501
    exec(
      `launchctl print gui/${uid}/com.meridiona.daemon`,
      { timeout: 3000 },
      (_err, stdout) => {
        if (!stdout) { resolve(undefined); return }
        // launchctl print shows "state = running" when live, "state = not running" otherwise.
        resolve(stdout.includes('state = running'))
      },
    )
  })
}

async function refresh(): Promise<void> {
  const [db, logTrust, daemonRunning] = await Promise.all([
    checkDatabase(),
    checkA11yTrusted(),
    checkDaemonRunning(),
  ])
  const trusted = logTrust !== undefined ? logTrust : await launchctlA11yTrusted()

  cache = {
    result: {
      database_ready: db.ready,
      ...(db.error ? { error: db.error } : {}),
      ...(trusted !== undefined ? { a11y_helper_trusted: trusted } : {}),
      ...(daemonRunning !== undefined ? { daemon_running: daemonRunning } : {}),
    },
    at: Date.now(),
  }
  inFlight = null
}

function scheduleRefresh(): void {
  if (inFlight) return
  inFlight = refresh().catch(() => { inFlight = null })
}

export async function GET(): Promise<NextResponse<HealthStatus>> {
  const stale = !cache || Date.now() - cache.at >= CACHE_TTL_MS
  if (stale) scheduleRefresh()
  // Always return immediately — {} on first call (banner hidden until data arrives),
  // stale cache on subsequent calls while a refresh runs in the background.
  return NextResponse.json(cache?.result ?? {})
}
