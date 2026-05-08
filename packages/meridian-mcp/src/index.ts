#!/usr/bin/env node
// meridian — normalises screenpipe activity into structured app sessions

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  ListResourcesRequestSchema,
  ReadResourceRequestSchema,
  Tool,
} from "@modelcontextprotocol/sdk/types.js";
import initSqlJs from "sql.js";
import * as os from "os";
import * as path from "path";
import * as fs from "fs";
import { runInstaller } from "./install.js";

type SqlJsStatic = Awaited<ReturnType<typeof initSqlJs>>;
type SqlDatabase = InstanceType<SqlJsStatic["Database"]>;
type SqlVal = string | number | null | Uint8Array;

function getDbPath(): string {
  return process.env.MERIDIAN_DB ?? path.join(os.homedir(), ".meridian", "meridian.db");
}

let _SQL: SqlJsStatic | null = null;

async function getSqlEngine(): Promise<SqlJsStatic> {
  if (!_SQL) {
    _SQL = await initSqlJs();
  }
  return _SQL;
}

async function openDb(): Promise<SqlDatabase> {
  const dbPath = getDbPath();
  if (!fs.existsSync(dbPath)) {
    throw new Error(`Meridian DB not found at ${dbPath}. Is the Meridian daemon running?`);
  }
  const SQL = await getSqlEngine();
  const fileBuffer = fs.readFileSync(dbPath);
  return new SQL.Database(fileBuffer);
}

function dbGet(db: SqlDatabase, sql: string, params: SqlVal[] = []): Record<string, unknown> | undefined {
  const stmt = db.prepare(sql);
  try {
    stmt.bind(params);
    if (!stmt.step()) return undefined;
    return stmt.getAsObject() as Record<string, unknown>;
  } finally {
    stmt.free();
  }
}

function dbAll(db: SqlDatabase, sql: string, params: SqlVal[] = []): Record<string, unknown>[] {
  const stmt = db.prepare(sql);
  const rows: Record<string, unknown>[] = [];
  try {
    stmt.bind(params);
    while (stmt.step()) {
      rows.push(stmt.getAsObject() as Record<string, unknown>);
    }
    return rows;
  } finally {
    stmt.free();
  }
}

function localDayBounds(dateStr: string): { start: string; end: string } {
  const [year, month, day] = dateStr.split("-").map(Number);
  const start = new Date(year, month - 1, day, 0, 0, 0, 0);
  const end = new Date(year, month - 1, day + 1, 0, 0, 0, 0);
  return { start: start.toISOString(), end: end.toISOString() };
}

function todayString(): string {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
}

function parseJsonColumn<T>(val: string | null | undefined): T | null {
  if (!val) return null;
  try { return JSON.parse(val) as T; } catch { return null; }
}

const PKG_VERSION = "1.0.0";

const server = new Server(
  { name: "meridian", version: PKG_VERSION },
  { capabilities: { tools: {}, resources: {} } }
);

const TOOLS: Tool[] = [
  {
    name: "get-sessions",
    description:
      "List completed app sessions for a given date. Each session includes app name, duration, " +
      "window titles, OCR text visible on screen, audio snippets, and accessibility elements. " +
      "Use this to answer: 'what apps did I use today?', 'how long was I in VS Code?', 'what was I working on?'",
    annotations: { title: "Get Sessions", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: {
      type: "object",
      properties: {
        date: { type: "string", description: "Local date YYYY-MM-DD. Defaults to today." },
        app: { type: "string", description: "Filter by exact app name (e.g. 'code.visualstudio.com'). Case-sensitive." },
        page: { type: "integer", description: "Page number, 1-based. Default 1.", default: 1 },
        page_size: { type: "integer", description: "Results per page, max 50. Default 20.", default: 20 },
      },
    },
  },
  {
    name: "get-timeline",
    description:
      "Get the full day timeline: app sessions ordered chronologically plus user-idle and system-sleep gaps. " +
      "Use for: 'walk me through my day', 'what was I doing at 3pm?', 'how many breaks did I take?'",
    annotations: { title: "Get Timeline", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: {
      type: "object",
      properties: {
        date: { type: "string", description: "Local date YYYY-MM-DD. Defaults to today." },
      },
    },
  },
  {
    name: "get-stats",
    description:
      "Get daily productivity stats: total focus time, idle time, away (sleep) time, session count, " +
      "and top 8 apps by duration. " +
      "Use for: 'how productive was I today?', 'what app did I spend the most time in?'",
    annotations: { title: "Get Stats", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: {
      type: "object",
      properties: {
        date: { type: "string", description: "Local date YYYY-MM-DD. Defaults to today." },
      },
    },
  },
  {
    name: "get-active-session",
    description:
      "Get the currently open app session (the app in focus right now). " +
      "Returns null if the daemon hasn't run recently or the computer is idle. " +
      "Use for: 'what am I doing right now?', 'what app is currently active?'",
    annotations: { title: "Get Active Session", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "get-apps",
    description:
      "Get all-time app usage statistics: total time, session count, average session length, and last-seen per app. " +
      "Use for: 'which apps do I use most overall?', 'when did I last use Slack?'",
    annotations: { title: "Get Apps", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: {
      type: "object",
      properties: {
        limit: { type: "integer", description: "Max apps to return. Default 20.", default: 20 },
      },
    },
  },
  {
    name: "search-sessions",
    description:
      "Search app sessions by text content: window titles, OCR text on screen, or audio transcriptions. " +
      "Use for: 'when did I work on the Stripe integration?', 'find sessions where I was in a meeting', " +
      "'which sessions mention authentication?'",
    annotations: { title: "Search Sessions", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: {
      type: "object",
      properties: {
        q: { type: "string", description: "Text to search for (case-insensitive, partial match). Searched across window titles, OCR text, audio transcriptions." },
        date: { type: "string", description: "Limit search to a specific date (YYYY-MM-DD). Omit to search all history." },
        app: { type: "string", description: "Filter by app name (case-sensitive, exact match)." },
        limit: { type: "integer", description: "Max results. Default 10.", default: 10 },
      },
      required: ["q"],
    },
  },
  {
    name: "get-session-detail",
    description:
      "Get full detail for a specific session by ID: all window titles, OCR text samples, audio transcriptions, " +
      "accessibility elements, and signals (clipboard, app switches). " +
      "Use after get-sessions or search-sessions when you need the full content of a specific session.",
    annotations: { title: "Get Session Detail", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: {
      type: "object",
      properties: {
        id: { type: "integer", description: "Session ID (from get-sessions or search-sessions results)." },
      },
      required: ["id"],
    },
  },
  {
    name: "health-check",
    description:
      "Check if the Meridian daemon is running and the DB is accessible. " +
      "Returns last ETL run status, cursor position, and total session count.",
    annotations: { title: "Health Check", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: { type: "object", properties: {} },
  },
];

server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: TOOLS }));

const RESOURCES = [
  {
    uri: "meridian://context",
    name: "Current Context",
    description: "Current date/time, timezone, and pre-computed date strings for common ranges",
    mimeType: "application/json",
  },
  {
    uri: "meridian://guide",
    name: "Usage Guide",
    description: "How to use Meridian tools effectively — tool selection and common patterns",
    mimeType: "text/markdown",
  },
];

server.setRequestHandler(ListResourcesRequestSchema, async () => ({ resources: RESOURCES }));

server.setRequestHandler(ReadResourceRequestSchema, async (request) => {
  const { uri } = request.params;

  if (uri === "meridian://context") {
    const now = new Date();
    const ms = now.getTime();
    return {
      contents: [{
        uri,
        mimeType: "application/json",
        text: JSON.stringify({
          current_time: now.toISOString(),
          current_date_local: now.toLocaleDateString("en-US", {
            weekday: "long", year: "numeric", month: "long", day: "numeric",
          }),
          today: todayString(),
          timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
          dates: {
            today: todayString(),
            yesterday: new Date(ms - 24 * 60 * 60 * 1000).toISOString().slice(0, 10),
            one_week_ago: new Date(ms - 7 * 24 * 60 * 60 * 1000).toISOString().slice(0, 10),
          },
        }, null, 2),
      }],
    };
  }

  if (uri === "meridian://guide") {
    return {
      contents: [{
        uri,
        mimeType: "text/markdown",
        text: `# Meridian Usage Guide

Meridian tracks your app usage by reading screenpipe's ambient recordings and normalising them into structured app sessions.

## Tool Selection

| Question | Tool |
|----------|------|
| "What did I do today?" | get-timeline |
| "How productive was I?" | get-stats |
| "Which apps did I use?" | get-sessions |
| "What am I doing right now?" | get-active-session |
| "When did I work on X?" | search-sessions with q=X |
| "Full content of a session?" | get-session-detail with id |
| "All-time app usage?" | get-apps |

## Tips

- **Dates** are local calendar dates (YYYY-MM-DD). Today is the default when omitted.
- **App names** are case-sensitive exact strings — use values from session results (e.g. "code.visualstudio.com").
- **search-sessions** searches window titles, OCR screen text, and audio transcriptions.
- **get-timeline** includes idle and sleep gaps for a full picture of the day.
- **get-session-detail** returns the full OCR text, audio, and accessibility content for a session.
- The Meridian daemon runs every 60 seconds — data may be up to 60s stale.
`,
      }],
    };
  }

  throw new Error(`Unknown resource: ${uri}`);
});

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;
  let db: SqlDatabase | undefined;

  try {
    db = await openDb();

    switch (name) {
      case "get-sessions": {
        const date = (args?.date as string) ?? todayString();
        const appFilter = args?.app as string | undefined;
        const page = Math.max(1, (args?.page as number) ?? 1);
        const pageSize = Math.min(50, (args?.page_size as number) ?? 20);
        const offset = (page - 1) * pageSize;
        const { start, end } = localDayBounds(date);

        const appCond = appFilter ? "AND app_name = ?" : "";
        const baseParams: SqlVal[] = appFilter ? [start, end, appFilter] : [start, end];

        const total = (dbGet(db, `
          SELECT COUNT(*) AS n FROM app_sessions
          WHERE started_at >= ? AND started_at < ? ${appCond}
        `, baseParams) as { n: number } | undefined)?.n ?? 0;

        const rows = dbAll(db, `
          SELECT id, app_name, started_at, ended_at, duration_s,
                 window_titles, frame_count, etl_run_id
          FROM app_sessions
          WHERE started_at >= ? AND started_at < ? ${appCond}
          ORDER BY started_at DESC
          LIMIT ? OFFSET ?
        `, [...baseParams, pageSize, offset]);

        if (rows.length === 0) {
          return { content: [{ type: "text", text: `No sessions for ${date}${appFilter ? ` (${appFilter})` : ""}.` }] };
        }

        const formatted = rows.map((r) => {
          const titles = parseJsonColumn<Array<{ window_name: string; count: number }>>(r.window_titles as string);
          const topTitles = titles?.slice(0, 3).map((t) => t.window_name).filter(Boolean).join(", ") ?? "";
          const durMin = Math.round((r.duration_s as number) / 60);
          const tStart = (r.started_at as string).slice(11, 16);
          const tEnd = (r.ended_at as string).slice(11, 16);
          return `[#${r.id}] ${r.app_name} | ${tStart}–${tEnd} UTC (${durMin}min, ${r.frame_count} frames)\n  Windows: ${topTitles || "(none)"}`;
        });

        const hasMore = offset + rows.length < total;
        const header = `Sessions for ${date}: ${rows.length}/${total}${hasMore ? ` — use page=${page + 1} for more` : ""}`;
        return { content: [{ type: "text", text: `${header}\n\n${formatted.join("\n---\n")}` }] };
      }

      case "get-timeline": {
        const date = (args?.date as string) ?? todayString();
        const { start, end } = localDayBounds(date);

        const sessions = dbAll(db, `
          SELECT id, app_name, started_at, ended_at, duration_s, window_titles, frame_count
          FROM app_sessions
          WHERE started_at >= ? AND started_at < ?
          ORDER BY started_at ASC
        `, [start, end]);

        const gaps = dbAll(db, `
          SELECT id, started_at, ended_at, duration_s, kind
          FROM gaps
          WHERE started_at >= ? AND started_at < ?
          ORDER BY started_at ASC
        `, [start, end]);

        if (sessions.length === 0 && gaps.length === 0) {
          return { content: [{ type: "text", text: `No data recorded for ${date}.` }] };
        }

        type Item = { started_at: string; ended_at: string; duration_s: number; _type: string; app_name?: string; window_titles?: string; kind?: string; frame_count?: number };

        const items: Item[] = [
          ...sessions.map((s) => ({ _type: "session", ...s } as Item)),
          ...gaps.map((g) => ({ _type: "gap", ...g } as Item)),
        ].sort((a, b) => a.started_at.localeCompare(b.started_at));

        const lines = items.map((item) => {
          const tS = item.started_at.slice(11, 16);
          const tE = item.ended_at.slice(11, 16);
          const dur = Math.round(item.duration_s / 60);
          if (item._type === "session") {
            const titles = parseJsonColumn<Array<{ window_name: string }>>(item.window_titles ?? null);
            const top = titles?.[0]?.window_name ?? "";
            return `${tS}–${tE} [APP] ${item.app_name} (${dur}min)${top ? `\n  └─ ${top}` : ""}`;
          }
          return `${tS}–${tE} [${(item.kind ?? "idle").toUpperCase()}] ${dur}min`;
        });

        const focusS = sessions.reduce((s, r) => s + (r.duration_s as number), 0);
        const idleS = gaps.filter(g => g.kind === "user_idle").reduce((s, r) => s + (r.duration_s as number), 0);
        const awayS = gaps.filter(g => g.kind === "system_sleep").reduce((s, r) => s + (r.duration_s as number), 0);

        const header = `Timeline for ${date} — Focus: ${Math.round(focusS / 60)}min | Idle: ${Math.round(idleS / 60)}min | Away: ${Math.round(awayS / 60)}min`;
        return { content: [{ type: "text", text: `${header}\n\n${lines.join("\n")}` }] };
      }

      case "get-stats": {
        const date = (args?.date as string) ?? todayString();
        const { start, end } = localDayBounds(date);

        const focusRow = (dbGet(db, `
          SELECT COALESCE(SUM(duration_s), 0) AS focus_s, COUNT(*) AS session_count
          FROM app_sessions WHERE started_at >= ? AND started_at < ?
        `, [start, end]) ?? { focus_s: 0, session_count: 0 }) as { focus_s: number; session_count: number };

        const idleS = ((dbGet(db, `
          SELECT COALESCE(SUM(duration_s), 0) AS s FROM gaps
          WHERE started_at >= ? AND started_at < ? AND kind = 'user_idle'
        `, [start, end]) ?? { s: 0 }) as { s: number }).s;

        const awayS = ((dbGet(db, `
          SELECT COALESCE(SUM(duration_s), 0) AS s FROM gaps
          WHERE started_at >= ? AND started_at < ? AND kind = 'system_sleep'
        `, [start, end]) ?? { s: 0 }) as { s: number }).s;

        const topApps = dbAll(db, `
          SELECT app_name, SUM(duration_s) AS total_s, COUNT(*) AS session_count
          FROM app_sessions WHERE started_at >= ? AND started_at < ?
          GROUP BY app_name ORDER BY total_s DESC LIMIT 8
        `, [start, end]) as Array<{ app_name: string; total_s: number; session_count: number }>;

        const appLines = topApps.map((a, i) =>
          `  ${i + 1}. ${a.app_name}: ${Math.round(a.total_s / 60)}min (${a.session_count} sessions)`
        );

        const text = [
          `Stats for ${date}`,
          `Focus: ${Math.round(focusRow.focus_s / 60)}min | Idle: ${Math.round(idleS / 60)}min | Away: ${Math.round(awayS / 60)}min`,
          `Sessions: ${focusRow.session_count}`,
          "",
          "Top apps:",
          ...(appLines.length ? appLines : ["  (none)"]),
        ].join("\n");

        return { content: [{ type: "text", text }] };
      }

      case "get-active-session": {
        const row = dbGet(db, `
          SELECT app_name, started_at, last_seen_at, window_titles,
                 ocr_samples, audio_snippets, frame_count
          FROM active_session WHERE id = 1
        `);

        if (!row) {
          return { content: [{ type: "text", text: "No active session. The Meridian daemon may not have run recently." }] };
        }

        const elapsed = Math.round((Date.now() - new Date(row.last_seen_at as string).getTime()) / 1000);
        const titles = parseJsonColumn<Array<{ window_name: string }>>(row.window_titles as string);
        const topTitle = titles?.[0]?.window_name ?? "";
        const audio = parseJsonColumn<Array<{ transcription: string }>>(row.audio_snippets as string);
        const lastAudio = audio?.at(-1)?.transcription ?? "";

        const lines = [
          `Active: ${row.app_name}`,
          `Started: ${(row.started_at as string).slice(11, 19)} UTC`,
          `Last seen: ${elapsed}s ago`,
          `Frames: ${row.frame_count}`,
          topTitle ? `Window: ${topTitle}` : "",
          lastAudio ? `Last audio: "${lastAudio}"` : "",
        ].filter(Boolean);

        return { content: [{ type: "text", text: lines.join("\n") }] };
      }

      case "get-apps": {
        const limit = Math.min(50, (args?.limit as number) ?? 20);

        const rows = dbAll(db, `
          SELECT app_name,
                 SUM(duration_s) AS total_s,
                 COUNT(*) AS session_count,
                 ROUND(AVG(duration_s), 0) AS avg_session_s,
                 MAX(ended_at) AS last_seen
          FROM app_sessions
          GROUP BY app_name
          ORDER BY total_s DESC
          LIMIT ?
        `, [limit]) as Array<{ app_name: string; total_s: number; session_count: number; avg_session_s: number; last_seen: string }>;

        if (rows.length === 0) {
          return { content: [{ type: "text", text: "No app data yet." }] };
        }

        const formatted = rows.map((r, i) => {
          const totalH = Math.round(r.total_s / 360) / 10;
          const avgMin = Math.round(r.avg_session_s / 60);
          return `${i + 1}. ${r.app_name}\n   Total: ${totalH}h | Sessions: ${r.session_count} | Avg: ${avgMin}min | Last: ${r.last_seen.slice(0, 10)}`;
        });

        return { content: [{ type: "text", text: `All-time app usage (top ${rows.length}):\n\n${formatted.join("\n")}` }] };
      }

      case "search-sessions": {
        const q = args?.q as string;
        if (!q) return { content: [{ type: "text", text: "Error: q is required" }] };

        const appFilter = args?.app as string | undefined;
        const date = args?.date as string | undefined;
        const limit = Math.min(50, (args?.limit as number) ?? 10);
        const pattern = `%${q}%`;

        let sql = `
          SELECT id, app_name, started_at, ended_at, duration_s, window_titles
          FROM app_sessions
          WHERE (window_titles LIKE ? OR ocr_samples LIKE ? OR audio_snippets LIKE ?)
        `;
        const params: SqlVal[] = [pattern, pattern, pattern];

        if (date) {
          const { start, end } = localDayBounds(date);
          sql += " AND started_at >= ? AND started_at < ?";
          params.push(start, end);
        }
        if (appFilter) {
          sql += " AND app_name = ?";
          params.push(appFilter);
        }
        sql += " ORDER BY started_at DESC LIMIT ?";
        params.push(limit);

        const rows = dbAll(db, sql, params);

        if (rows.length === 0) {
          return { content: [{ type: "text", text: `No sessions found containing "${q}".` }] };
        }

        const formatted = rows.map((r) => {
          const titles = parseJsonColumn<Array<{ window_name: string }>>(r.window_titles as string);
          const topTitle = titles?.[0]?.window_name ?? "";
          const durMin = Math.round((r.duration_s as number) / 60);
          const started = (r.started_at as string).slice(0, 16).replace("T", " ");
          return `[#${r.id}] ${r.app_name} | ${started} UTC (${durMin}min)\n  ${topTitle}`;
        });

        return { content: [{ type: "text", text: `Found ${rows.length} session(s) matching "${q}":\n\n${formatted.join("\n---\n")}` }] };
      }

      case "get-session-detail": {
        const id = args?.id as number;
        if (!id) return { content: [{ type: "text", text: "Error: id is required" }] };

        const row = dbGet(db, `
          SELECT id, app_name, started_at, ended_at, duration_s,
                 window_titles, ocr_samples, elements_samples, audio_snippets, signals,
                 frame_count, idle_frame_count, etl_run_id
          FROM app_sessions WHERE id = ?
        `, [id]);

        if (!row) {
          return { content: [{ type: "text", text: `Session #${id} not found.` }] };
        }

        const durMin = Math.round((row.duration_s as number) / 60);
        const titles = parseJsonColumn<Array<{ window_name: string; count: number }>>(row.window_titles as string);
        const ocr = parseJsonColumn<Array<{ text: string; window_name: string; timestamp: string }>>(row.ocr_samples as string);
        const audio = parseJsonColumn<Array<{ transcription: string; timestamp: string; speaker_id: number | null }>>(row.audio_snippets as string);
        const elements = parseJsonColumn<Array<{ text: string; role: string; window_name: string }>>(row.elements_samples as string);
        const signals = parseJsonColumn<Array<{ event_type: string; value: string; timestamp: string }>>(row.signals as string);

        const lines: string[] = [
          `Session #${row.id}: ${row.app_name}`,
          `${(row.started_at as string).slice(11, 19)}–${(row.ended_at as string).slice(11, 19)} UTC | ${durMin}min | ${row.frame_count} frames`,
          "",
        ];

        if (titles?.length) {
          lines.push("Window titles:");
          titles.slice(0, 10).forEach((t) => lines.push(`  ${t.count}x ${t.window_name}`));
          lines.push("");
        }

        if (ocr?.length) {
          lines.push(`OCR samples (${ocr.length}):`);
          ocr.slice(0, 5).forEach((o) => lines.push(`  [${o.timestamp.slice(11, 19)}] ${o.text.slice(0, 200)}`));
          lines.push("");
        }

        if (audio?.length) {
          lines.push(`Audio (${audio.length} snippets):`);
          audio.forEach((a) => lines.push(`  [${a.timestamp.slice(11, 19)}] ${a.transcription}`));
          lines.push("");
        }

        if (elements?.length) {
          lines.push(`Accessibility elements (${elements.length} samples):`);
          elements.slice(0, 5).forEach((e) => lines.push(`  [${e.role}] ${e.text?.slice(0, 150)}`));
          lines.push("");
        }

        if (signals?.length) {
          lines.push(`Signals (${signals.length}):`);
          signals.forEach((s) => lines.push(`  [${s.event_type}] ${s.value?.slice(0, 100)}`));
        }

        return { content: [{ type: "text", text: lines.join("\n") }] };
      }

      case "health-check": {
        const dbPath = getDbPath();

        const lastRun = dbGet(db, `
          SELECT id, started_at, completed_at, status, sessions_closed, from_frame_id, to_frame_id
          FROM etl_runs ORDER BY id DESC LIMIT 1
        `);

        const cursor = dbGet(db, `
          SELECT last_frame_id, last_run_at FROM etl_cursor WHERE id = 1
        `);

        const sessionCount = (dbGet(db, `SELECT COUNT(*) AS n FROM app_sessions`) as { n: number } | undefined)?.n ?? 0;

        const lines = [
          `DB: ${dbPath}`,
          `Total sessions: ${sessionCount}`,
          "",
          lastRun
            ? `Last ETL: #${lastRun.id} | ${lastRun.status} | completed ${(lastRun.completed_at as string)?.slice(11, 19)} UTC | ${lastRun.sessions_closed} sessions closed | frames ${lastRun.from_frame_id}→${lastRun.to_frame_id}`
            : "No ETL runs found — is the daemon running?",
          cursor
            ? `Cursor: frame #${cursor.last_frame_id}, last run at ${(cursor.last_run_at as string)?.slice(11, 19)} UTC`
            : "No cursor — daemon has not run yet.",
        ];

        return { content: [{ type: "text", text: lines.join("\n") }] };
      }

      default:
        throw new Error(`Unknown tool: ${name}`);
    }
  } catch (error) {
    const msg = error instanceof Error ? error.message : String(error);
    return { content: [{ type: "text", text: `Error: ${msg}` }] };
  } finally {
    db?.close();
  }
});

async function main() {
  if (process.argv[2] === "install") {
    runInstaller();
    return;
  }
  const transport = new StdioServerTransport();
  await server.connect(transport);
  console.error("Meridian MCP server running on stdio");
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
