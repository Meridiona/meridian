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
import * as os from "os";
import * as path from "path";
import * as crypto from "crypto";
import { execFile } from "child_process";
import { promisify } from "util";

const execFileAsync = promisify(execFile);

// meridian.db may be SQLCipher-encrypted at rest (see meridian-core's
// db_crypto.rs). sql.js is pure WASM SQLite with no SQLCipher support, so it
// cannot open an encrypted file directly — instead this module shells out to
// `meridian db-export-plaintext`, which decrypts to a plaintext snapshot this
// package then loads exactly as before. Detected via the plain SQLite file
// magic header: an encrypted file has no recognizable header at all.
const SQLITE_MAGIC = Uint8Array.from(Buffer.from("SQLite format 3\0", "latin1"));

function isPlaintextSqlite(filePath: string): boolean {
  const fd = fs.openSync(filePath, "r");
  try {
    const header = new Uint8Array(16);
    const bytesRead = fs.readSync(fd, header, 0, 16, 0);
    return (
      bytesRead === 16 && header.every((byte, i) => byte === SQLITE_MAGIC[i])
    );
  } finally {
    fs.closeSync(fd);
  }
}

/** Candidate paths for the `meridian` CLI, native binary first (no Node/PATH
 * dependency — matches how the tray stages it, see backend_install.rs), bare
 * `meridian` last as a PATH-reliant fallback for source/dev setups. */
function meridianCandidates(): string[] {
  const home = os.homedir();
  return [path.join(home, ".meridian", "bin", "meridian"), "meridian"];
}

function selectMeridianBinary(candidates: string[]): string {
  for (const candidate of candidates) {
    if (path.isAbsolute(candidate) && fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return candidates[candidates.length - 1];
}

/**
 * Resolves `MERIDIAN_DB_KEY` the same way `getDbPath()` resolves `MERIDIAN_DB`:
 * an explicit env var first (settable in the MCP client's server config,
 * exactly like `MERIDIAN_DB`), falling back to reading it out of the tray's
 * canonical `.env` (`~/.meridian/.env`) — this package never loads `.env`
 * files wholesale, it only ever reads the one key it needs, same as the
 * tray's own `env_key_from_path` (`install.rs`).
 */
function getDbKey(): string | undefined {
  if (process.env.MERIDIAN_DB_KEY) return process.env.MERIDIAN_DB_KEY;
  const envPath = path.join(os.homedir(), ".meridian", ".env");
  try {
    const contents = fs.readFileSync(envPath, "utf8");
    for (const line of contents.split("\n")) {
      const trimmed = line.trim();
      if (trimmed.startsWith("MERIDIAN_DB_KEY=")) {
        return trimmed.slice("MERIDIAN_DB_KEY=".length).trim();
      }
    }
  } catch {
    // No canonical .env (dev/source install, or not yet provisioned) — the DB
    // is simply unencrypted in that case; isPlaintextSqlite() below is what
    // actually decides whether an export is needed at all.
  }
  return undefined;
}

/**
 * If `dbPath` is SQLCipher-encrypted, exports a plaintext snapshot (via the
 * `meridian` CLI) to a stable per-path temp file and returns that path;
 * otherwise returns `dbPath` unchanged. Throws with a clear message if the
 * export fails or no key can be resolved.
 */
async function resolveReadablePath(dbPath: string): Promise<string> {
  if (isPlaintextSqlite(dbPath)) {
    return dbPath;
  }
  const key = getDbKey();
  if (!key) {
    throw new Error(
      `Meridian DB at ${dbPath} is encrypted, but no MERIDIAN_DB_KEY could be resolved ` +
        `(checked the MERIDIAN_DB_KEY env var and ~/.meridian/.env). Set MERIDIAN_DB_KEY ` +
        `in this MCP server's environment, matching the key the Meridian tray generated.`,
    );
  }
  // Hash the WHOLE path. The previous `Buffer.from(dbPath).toString("hex")
  // .slice(0, 16)` was the hex of only the first EIGHT bytes, so any two DB
  // paths sharing an 8-byte prefix mapped to the same snapshot file - and
  // "/Users/a" is exactly eight bytes, which makes `/Users/alice/...` and
  // `/Users/alison/...` collide. Since the caches key on `dbPath` rather than
  // on this derived path, two paths can be resolved concurrently, and a
  // collision lets one database's export overwrite the other between its
  // export and its read - serving one user's data for a request against
  // another's.
  const snapshotPath = path.join(
    os.tmpdir(),
    `meridian-mcp-plaintext-${crypto.createHash("sha256").update(dbPath).digest("hex").slice(0, 32)}.db`,
  );
  const bin = selectMeridianBinary(meridianCandidates());
  try {
    await execFileAsync(bin, ["db-export-plaintext", "--out", snapshotPath], {
      env: { ...process.env, MERIDIAN_DB: dbPath, MERIDIAN_DB_KEY: key },
    });
  } catch (err) {
    throw new Error(
      `Meridian DB at ${dbPath} is encrypted and could not be exported for reading via '${bin} db-export-plaintext': ${
        err instanceof Error ? err.message : String(err)
      }`,
    );
  }
  return snapshotPath;
}

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
  if (!stat.isFile()) {
    // existsSync also accepts directories, FIFOs, and devices; readFileSync on
    // any of those would fail obscurely (or block). Fail with a clear config error.
    throw new Error(`Meridian DB path is not a regular file: ${dbPath}`);
  }
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
      const readablePath = await resolveReadablePath(dbPath);
      const fileBuffer = fs.readFileSync(readablePath);
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
