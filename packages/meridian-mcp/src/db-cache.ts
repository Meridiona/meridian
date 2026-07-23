//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Cached sql.js Database loader.
//
// sql.js is pure WASM (no native compile step) — deliberately chosen over
// better-sqlite3 for this package specifically because native-module ABI
// mismatches across Node versions/platforms caused real distribution pain
// elsewhere in this project's history. Do not swap this for better-sqlite3;
// see the memory-leak-audit fix that added this module for the full context.
//
// Without caching, every MCP tool call did fs.readFileSync(dbPath) (one full
// copy of meridian.db into a Node Buffer) followed by `new SQL.Database(...)`
// (a second full copy into WASM linear memory), and emscripten never returns
// freed WASM pages to the OS — so process RSS climbed to the size of the
// largest DB ever loaded and stayed there, and got worse every tool call.
//
// This module reuses a single long-lived Database instance per dbPath across
// calls, invalidating (re-reading the file, rebuilding the WASM instance)
// only when the file's mtime or size changes — a cheap `stat()` per call
// instead of a full read + WASM rebuild. The daemon is a separate process
// that writes meridian.db; tool calls are read-mostly, so a small staleness
// window between a daemon write and the next stat-triggered reload is
// acceptable in exchange for not re-paying the double-copy cost on every call.

import initSqlJs from "sql.js";
import * as fs from "fs";

export type SqlJsStatic = Awaited<ReturnType<typeof initSqlJs>>;
export type SqlDatabase = InstanceType<SqlJsStatic["Database"]>;

interface CachedDb {
  db: SqlDatabase;
  mtimeMs: number;
  size: number;
}

let _SQL: SqlJsStatic | null = null;

/** One cached Database (+ the file stat it was loaded from) per dbPath. */
const _dbCache = new Map<string, CachedDb>();

/** Dedupes concurrent reload attempts for the same path onto one load. */
const _dbLoadInFlight = new Map<string, Promise<SqlDatabase>>();

async function getSqlEngine(): Promise<SqlJsStatic> {
  if (!_SQL) {
    _SQL = await initSqlJs();
  }
  return _SQL;
}

/**
 * Opens (or reuses) a cached sql.js `Database` for `dbPath`.
 *
 * Returns the cached instance as long as the file's `mtimeMs`/`size` (from a
 * cheap `fs.statSync`) match what was loaded last; otherwise re-reads the
 * file and rebuilds the WASM `Database`, closing the stale instance first —
 * sql.js/emscripten never returns freed WASM memory to the OS, so leaving the
 * old instance open would leak on every reload rather than just on every call.
 *
 * Throws if `dbPath` does not exist (mirrors the daemon-not-running check the
 * old per-call `openDb` performed).
 */
export async function openCachedDb(dbPath: string): Promise<SqlDatabase> {
  if (!fs.existsSync(dbPath)) {
    throw new Error(`Meridian DB not found at ${dbPath}. Is the Meridian daemon running?`);
  }

  const stat = fs.statSync(dbPath);
  const cached = _dbCache.get(dbPath);
  if (cached && cached.mtimeMs === stat.mtimeMs && cached.size === stat.size) {
    return cached.db;
  }

  // Reuse an in-flight reload for this path instead of racing a second one
  // if two tool calls both observe a stale cache before either finishes.
  const inFlight = _dbLoadInFlight.get(dbPath);
  if (inFlight) return inFlight;

  const loadPromise = (async () => {
    try {
      const SQL = await getSqlEngine();
      const fileBuffer = fs.readFileSync(dbPath);
      const db = new SQL.Database(fileBuffer);

      const prev = _dbCache.get(dbPath);
      prev?.db.close();

      _dbCache.set(dbPath, { db, mtimeMs: stat.mtimeMs, size: stat.size });
      return db;
    } finally {
      _dbLoadInFlight.delete(dbPath);
    }
  })();
  _dbLoadInFlight.set(dbPath, loadPromise);
  return loadPromise;
}

/** Test-only: closes and clears every cached instance. */
export function _resetDbCacheForTests(): void {
  for (const { db } of _dbCache.values()) {
    db.close();
  }
  _dbCache.clear();
  _dbLoadInFlight.clear();
}

/** Test-only: number of distinct paths currently cached. */
export function _dbCacheSizeForTests(): number {
  return _dbCache.size;
}
