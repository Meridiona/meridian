// meridian — normalises screenpipe activity into structured app sessions
//
// SSE endpoint for real-time health status. Uses fs.watch (FSEvents on macOS)
// to detect DB and a11y-helper log changes — no polling, instant notification.
// ReadableStream.start() runs after the Response is returned, so getHealth()
// never blocks the caller.

import { watch } from 'fs'
import { access, constants, readFile } from 'fs/promises'
import { exec } from 'child_process'
import path from 'path'
import os from 'os'

export const dynamic = 'force-dynamic'

interface HealthStatus {
  a11y_helper_trusted?: boolean
  database_ready?: boolean
  error?: string
}

function dbPath(): string {
  return process.env.MERIDIAN_DB_PATH ?? path.join(os.homedir(), '.meridian', 'meridian.db')
}

function logsDir(): string {
  return path.join(os.homedir(), '.meridian', 'logs')
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

async function checkA11yFromLog(): Promise<boolean | undefined> {
  const logFile = path.join(logsDir(), 'a11y-helper.log')
  try {
    const raw = await readFile(logFile, 'utf8')
    const lines = raw.trimEnd().split('\n')
    for (let i = lines.length - 1; i >= Math.max(0, lines.length - 200); i--) {
      const l = lines[i]
      if (l.includes('trusted: true') || l.includes('[trusted]')) return true
      if (l.includes('trusted: false') || l.includes('[untrusted]')) return false
    }
    return undefined
  } catch {
    return undefined
  }
}

function launchctlA11yTrusted(): Promise<boolean | undefined> {
  return new Promise((resolve) => {
    const uid = process.getuid?.() ?? 501
    exec(`launchctl print gui/${uid}/com.meridiona.a11y-helper`, { timeout: 3000 }, (_err, stdout) => {
      if (!stdout) { resolve(undefined); return }
      if (stdout.includes('a11y_trusted = 1')) resolve(true)
      else if (stdout.includes('a11y_trusted = 0')) resolve(false)
      else resolve(undefined)
    })
  })
}

async function getHealth(): Promise<HealthStatus> {
  const [db, logTrust] = await Promise.all([checkDatabase(), checkA11yFromLog()])
  const trusted = logTrust !== undefined ? logTrust : await launchctlA11yTrusted()
  return {
    database_ready: db.ready,
    ...(db.error ? { error: db.error } : {}),
    ...(trusted !== undefined ? { a11y_helper_trusted: trusted } : {}),
  }
}

export async function GET(request: Request) {
  const encoder = new TextEncoder()

  const stream = new ReadableStream({
    async start(controller) {
      const enqueue = (data: HealthStatus) => {
        try { controller.enqueue(encoder.encode(`data: ${JSON.stringify(data)}\n\n`)) } catch { /* closed */ }
      }
      const ping = () => {
        try { controller.enqueue(encoder.encode(': ping\n\n')) } catch { /* closed */ }
      }

      // Initial health push — runs after the Response has been returned.
      enqueue(await getHealth())

      // Coalesce rapid fs.watch events.
      let debounce: ReturnType<typeof setTimeout> | null = null
      const onChange = () => {
        if (debounce) clearTimeout(debounce)
        debounce = setTimeout(() => { getHealth().then(enqueue) }, 300)
      }

      const watchers: ReturnType<typeof watch>[] = []
      const meridianDir = path.dirname(dbPath())
      const dbFile = path.basename(dbPath())
      try {
        watchers.push(watch(meridianDir, (_, filename) => { if (filename === dbFile) onChange() }))
      } catch { /* dir absent */ }
      try {
        watchers.push(watch(logsDir(), (_, filename) => { if (filename === 'a11y-helper.log') onChange() }))
      } catch { /* logs dir absent */ }

      const heartbeatTimer = setInterval(ping, 25_000)

      const cleanup = () => {
        if (debounce) clearTimeout(debounce)
        clearInterval(heartbeatTimer)
        watchers.forEach((w) => { try { w.close() } catch { /* already closed */ } })
        try { controller.close() } catch { /* already closed */ }
      }

      request.signal.addEventListener('abort', cleanup)
    },
  })

  return new Response(stream, {
    headers: {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache, no-transform',
      'Connection': 'keep-alive',
      'X-Accel-Buffering': 'no',
    },
  })
}
