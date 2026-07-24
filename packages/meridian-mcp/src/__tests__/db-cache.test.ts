//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Tests for db-cache.ts's mtime/size-gated sql.js Database cache — the fix
// for the MCP server re-reading + re-allocating the whole meridian.db file
// on every single tool call. Run via `node --test` (built-in Node test
// runner, no extra test-framework dependency) against the compiled output:
// `npm run build && node --test dist/__tests__/`.

import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import initSqlJs from "sql.js";
import { openCachedDb, _resetDbCacheForTests, _dbCacheSizeForTests } from "../db-cache.js";

async function makeTempDb(dir: string, name: string, seedRows: number): Promise<string> {
  const SQL = await initSqlJs();
  const db = new SQL.Database();
  db.run("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)");
  for (let i = 0; i < seedRows; i++) {
    db.run("INSERT INTO t (val) VALUES (?)", [`row-${i}`]);
  }
  const bytes = db.export();
  db.close();
  const dbPath = path.join(dir, name);
  fs.writeFileSync(dbPath, bytes);
  return dbPath;
}

test("openCachedDb reuses the same instance when the file is unchanged", async () => {
  _resetDbCacheForTests();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "meridian-mcp-cache-test-"));
  try {
    const dbPath = await makeTempDb(dir, "meridian.db", 3);

    const first = await openCachedDb(dbPath);
    const second = await openCachedDb(dbPath);

    assert.strictEqual(second, first, "second call must return the exact same cached instance");
    assert.equal(_dbCacheSizeForTests(), 1, "exactly one path must be cached");
  } finally {
    _resetDbCacheForTests();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("openCachedDb reloads when the file's mtime/size changes", async () => {
  _resetDbCacheForTests();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "meridian-mcp-cache-test-"));
  try {
    const dbPath = await makeTempDb(dir, "meridian.db", 3);

    const first = await openCachedDb(dbPath);
    const firstRows = first.exec("SELECT COUNT(*) AS n FROM t")[0].values[0][0];
    assert.equal(firstRows, 3);

    // Rewrite the file with different content (different size) and bump
    // mtime forward so the cache's stat comparison is guaranteed to differ
    // even on filesystems with coarse mtime resolution.
    await makeTempDb(dir, "meridian.db", 7);
    const future = new Date(Date.now() + 5000);
    fs.utimesSync(dbPath, future, future);

    const second = await openCachedDb(dbPath);
    assert.notStrictEqual(second, first, "changed file must produce a new instance, not the cached one");

    const secondRows = second.exec("SELECT COUNT(*) AS n FROM t")[0].values[0][0];
    assert.equal(secondRows, 7, "reloaded instance must reflect the new file contents");
  } finally {
    _resetDbCacheForTests();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("openCachedDb throws when the file does not exist", async () => {
  _resetDbCacheForTests();
  await assert.rejects(
    () => openCachedDb("/nonexistent/path/does-not-exist.db"),
    /Meridian DB not found/,
  );
});

test("concurrent openCachedDb calls on a fresh path dedupe onto one load", async () => {
  _resetDbCacheForTests();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "meridian-mcp-cache-test-"));
  try {
    const dbPath = await makeTempDb(dir, "meridian.db", 1);

    const [a, b] = await Promise.all([openCachedDb(dbPath), openCachedDb(dbPath)]);
    assert.strictEqual(b, a, "concurrent calls for the same fresh path must resolve to one instance");
    assert.equal(_dbCacheSizeForTests(), 1);
  } finally {
    _resetDbCacheForTests();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
