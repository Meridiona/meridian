// meridian — normalises screenpipe activity into structured app sessions
//
// Writable meridian.db handle — used ONLY by the worklog approval mutations
// (edit / approve / reject). Everything else reads through the read-only handle
// in `db.ts`. The daemon also writes this DB (sqlx, WAL), so concurrent access
// is expected: WAL serialises writers and `busy_timeout` rides out the daemon's
// short write transactions. IMPORTANT: Node.js runtime only (API routes).

import Database from 'better-sqlite3'
import path from 'path'
import os from 'os'

declare global {
  // eslint-disable-next-line no-var
  var __meridian_write_db__: Database.Database | undefined
}

function expandTilde(filePath: string): string {
  if (filePath.startsWith('~/') || filePath === '~') {
    return path.join(os.homedir(), filePath.slice(2))
  }
  return filePath
}

export function getWriteDb(): Database.Database {
  if (globalThis.__meridian_write_db__) {
    return globalThis.__meridian_write_db__
  }

  const rawPath = process.env.MERIDIAN_DB_PATH ?? '~/.meridian/meridian.db'
  const db = new Database(expandTilde(rawPath), {
    readonly: false,
    fileMustExist: true,
  })

  db.pragma('journal_mode = WAL')
  db.pragma('busy_timeout = 5000')

  globalThis.__meridian_write_db__ = db
  return db
}
