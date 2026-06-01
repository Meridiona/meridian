// IMPORTANT: Only runs in Node.js runtime (API routes / Server Components).
// next.config.ts sets serverExternalPackages: ['better-sqlite3']

import Database from 'better-sqlite3'
import path from 'path'
import os from 'os'

declare global {
  // eslint-disable-next-line no-var
  var __meridian_db__: Database.Database | undefined
}

function expandTilde(filePath: string): string {
  if (filePath.startsWith('~/') || filePath === '~') {
    return path.join(os.homedir(), filePath.slice(2))
  }
  return filePath
}

function getDb(): Database.Database {
  // Cache the connection in ALL environments. In a long-running production
  // server, opening a fresh connection per request would leak file descriptors
  // (db + -wal + -shm) and page-cache memory until GC finalizes each handle —
  // the source of the unbounded RSS growth seen under request load. The
  // single read-only handle is safe to share across concurrent requests.
  if (globalThis.__meridian_db__) {
    return globalThis.__meridian_db__
  }

  const rawPath = process.env.MERIDIAN_DB_PATH ?? '~/.meridian/meridian.db'
  const resolvedPath = expandTilde(rawPath)

  const db = new Database(resolvedPath, {
    readonly: true,
    fileMustExist: true,
  })

  db.pragma('busy_timeout = 5000')

  globalThis.__meridian_db__ = db

  return db
}

export default getDb
