//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Singleton that tails the daemon's JSONL log file and broadcasts new entries
// to all open SSE connections. Uses fs.watch() (kqueue/inotify) on the log
// directory — reliable for append-only files unlike SQLite WAL.
//
// One watcher per process lifetime. N open browser tabs share one watcher and
// one file position — zero extra CPU per additional tab.

import fs from 'fs'
import path from 'path'
import os from 'os'
import readline from 'readline'

export interface LogEntry {
  timestamp: string
  level: string
  message: string
  target: string
  fields: Record<string, unknown>
  span?: string
}

type Controller = ReadableStreamDefaultController<Uint8Array>

const encoder = new TextEncoder()
const controllers = new Set<Controller>()

let watcherStarted = false
let currentLogPath = ''
let filePosition = 0

function logDir(): string {
  return process.env.MERIDIAN_LOG_DIR ?? path.join(os.homedir(), '.meridian', 'logs')
}

function todayLogPath(): string {
  const date = new Date().toISOString().slice(0, 10)
  return path.join(logDir(), `meridian-rust.jsonl.${date}`)
}

function parseLine(raw: string): LogEntry | null {
  try {
    const obj = JSON.parse(raw)
    return {
      timestamp: obj.timestamp ?? '',
      level: (obj.level ?? 'INFO').toUpperCase(),
      message: obj.fields?.message ?? obj.message ?? '',
      target: obj.target ?? '',
      span: obj.span?.name,
      fields: obj.fields ? (() => {
        // eslint-disable-next-line @typescript-eslint/no-unused-vars
        const { message: _m, scope: _s, ...rest } = obj.fields
        return rest
      })() : {},
    }
  } catch {
    return null
  }
}

function broadcast(entries: LogEntry[]) {
  if (entries.length === 0 || controllers.size === 0) return
  const payload = encoder.encode(`data: ${JSON.stringify(entries)}\n\n`)
  for (const ctrl of controllers) {
    try {
      ctrl.enqueue(payload)
    } catch {
      controllers.delete(ctrl)
    }
  }
}

function readNewLines() {
  const logPath = todayLogPath()

  // If the date rolled over, reset position for the new file
  if (logPath !== currentLogPath) {
    currentLogPath = logPath
    filePosition = 0
  }

  try {
    const stat = fs.statSync(logPath)
    if (stat.size <= filePosition) return
    const fd = fs.openSync(logPath, 'r')
    const buf = Buffer.alloc(stat.size - filePosition)
    fs.readSync(fd, buf as unknown as Uint8Array, 0, buf.length, filePosition)
    fs.closeSync(fd)
    filePosition = stat.size

    const lines = buf.toString('utf8').split('\n').filter(Boolean)
    const entries = lines.map(parseLine).filter((e): e is LogEntry => e !== null)
    broadcast(entries)
  } catch {
    // Log file not yet created (daemon not started today) — ignore
  }
}

function ensureWatcher() {
  if (watcherStarted) return
  watcherStarted = true
  currentLogPath = todayLogPath()

  // Watch the directory so we catch both writes to today's file and the moment
  // a new dated file appears at midnight
  try {
    fs.watch(logDir(), { persistent: false }, (_event, filename) => {
      if (filename?.startsWith('meridian-rust.jsonl')) {
        readNewLines()
      }
    })
  } catch {
    // Log dir not yet created — fall back to polling until it appears
    const poll = setInterval(() => {
      try {
        if (fs.existsSync(logDir())) {
          clearInterval(poll)
          ensureWatcher()
        }
      } catch { /* ignore */ }
    }, 5_000)
  }
}

/** Read the last `n` lines from the current or most-recent log file. */
export function readRecentLines(n: number): Promise<LogEntry[]> {
  return new Promise((resolve) => {
    // Try today's file first, fall back to yesterday's if today doesn't exist yet
    const candidates = [todayLogPath()]
    const yesterday = new Date(Date.now() - 86_400_000).toISOString().slice(0, 10)
    candidates.push(path.join(logDir(), `meridian-rust.jsonl.${yesterday}`))

    let logPath = ''
    for (const p of candidates) {
      if (fs.existsSync(p)) { logPath = p; break }
    }
    if (!logPath) return resolve([])

    const entries: LogEntry[] = []
    const rl = readline.createInterface({ input: fs.createReadStream(logPath) })
    rl.on('line', (line) => {
      const e = parseLine(line)
      if (e) entries.push(e)
    })
    rl.on('close', () => {
      // Set position so the SSE watcher picks up from here
      try { filePosition = fs.statSync(logPath).size } catch { /* ignore */ }
      currentLogPath = logPath
      resolve(entries.slice(-n))
    })
    rl.on('error', () => resolve([]))
  })
}

export function subscribeToTail(ctrl: Controller) {
  controllers.add(ctrl)
  ensureWatcher()
}

export function unsubscribeFromTail(ctrl: Controller) {
  controllers.delete(ctrl)
}
