#!/usr/bin/env node
// meridian — normalises screenpipe activity into structured app sessions

// Initialise OTel BEFORE any other imports create spans / tracers.
import { initOtel, logger, withSpan } from "./observability.js";
initOtel("meridian-mcp");

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

interface TimeWindow {
  fromTs: string;
  untilTs: string | null;
  cond: string;       // s.started_at-qualified, for queries with a JOIN alias
  plainCond: string;  // unqualified, for single-table queries
  params: SqlVal[];
  label: string;
}

// Priority order: from_time/to_time > since_hours > date (defaults to today)
function buildTimeWindow({ sinceHours, date, fromTime, toTime }: {
  sinceHours?: number;
  date?: string;
  fromTime?: string;  // ISO 8601 UTC — exact window start
  toTime?: string;    // ISO 8601 UTC — exact window end (optional)
}): TimeWindow {
  if (fromTime) {
    const fromTs = new Date(fromTime).toISOString();
    const toTs = toTime ? new Date(toTime).toISOString() : null;
    const label = toTs
      ? `${fromTs.slice(0, 16).replace("T", " ")}–${toTs.slice(11, 16)} UTC`
      : `from ${fromTs.slice(0, 16).replace("T", " ")} UTC`;
    return {
      fromTs, untilTs: toTs,
      cond: toTs ? "s.started_at >= ? AND s.started_at < ?" : "s.started_at >= ?",
      plainCond: toTs ? "started_at >= ? AND started_at < ?" : "started_at >= ?",
      params: toTs ? [fromTs, toTs] : [fromTs],
      label,
    };
  }
  if (sinceHours != null) {
    const h = Math.min(168, Math.max(0.5, sinceHours));
    const fromTs = new Date(Date.now() - h * 3600 * 1000).toISOString();
    return {
      fromTs, untilTs: null,
      cond: "s.started_at >= ?",
      plainCond: "started_at >= ?",
      params: [fromTs],
      label: `last ${h}h`,
    };
  }
  const d = date ?? todayString();
  const { start, end } = localDayBounds(d);
  return {
    fromTs: start, untilTs: end,
    cond: "s.started_at >= ? AND s.started_at < ?",
    plainCond: "started_at >= ? AND started_at < ?",
    params: [start, end],
    label: d,
  };
}

function fmtDuration(seconds: number): string {
  const m = Math.round(seconds / 60);
  return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : `${m}m`;
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
      "window titles, OCR text visible on screen, and accessibility elements. " +
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
      "Get full detail for a specific session by ID: all window titles, OCR text samples, " +
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
    name: "get-task-sessions",
    description:
      "Get all sessions linked to a specific Jira task key by the AI tagger pipeline (via ticket_links). " +
      "More precise than search-sessions: uses the tagger's semantic + rule-based classification, not just text search. " +
      "Optionally filter to a recent time window. " +
      "Use for: 'what did I do on KAN-108 today?', 'show all work on KAN-108 in the last 4 hours', " +
      "'how long have I spent on this ticket?', 'what was I actually doing on this task?'. " +
      "Set include_content=true to also return OCR and audio text from each session.",
    annotations: { title: "Get Task Sessions", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: {
      type: "object",
      properties: {
        task_key: { type: "string", description: "Jira task key, e.g. 'KAN-108'." },
        from_time: { type: "string", description: "Exact window start as ISO 8601 UTC, e.g. '2026-05-13T09:00:00Z'. Takes priority over since_hours and date." },
        to_time: { type: "string", description: "Exact window end as ISO 8601 UTC, e.g. '2026-05-13T13:00:00Z'. Used with from_time." },
        since_hours: { type: "number", description: "Return sessions from the last N hours (max 48). Overrides date when set." },
        date: { type: "string", description: "Limit to a specific date (YYYY-MM-DD). Defaults to today if no other time param is set." },
        include_content: { type: "boolean", description: "If true, include screen content (OCR + audio text) per session. Default false.", default: false },
      },
      required: ["task_key"],
    },
  },
  {
    name: "get-recent-sessions",
    description:
      "Get sessions from the last N hours across all apps, with any linked Jira task shown alongside each session. " +
      "Unlike get-sessions (which is date-based), this uses a sliding time window. " +
      "Use for: 'what did I do in the last 2 hours?', 'what was I working on since lunch?', " +
      "'give me a quick summary of recent activity', 'what task am I currently focused on?'.",
    annotations: { title: "Get Recent Sessions", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: {
      type: "object",
      properties: {
        hours: { type: "number", description: "Look-back window in hours. Default 4. Max 48.", default: 4 },
        app: { type: "string", description: "Filter by app name (case-sensitive, exact match)." },
        limit: { type: "integer", description: "Max sessions to return. Default 20.", default: 20 },
      },
    },
  },
  {
    name: "get-task-breakdown",
    description:
      "For a given date or recent time window, show total time spent grouped by linked Jira task. " +
      "Includes task title, status, URL, session count, and percentage of total focus time. " +
      "Use for: 'how much time per ticket today?', 'give me a work breakdown for today', " +
      "'what tasks did I work on this week?', 'how was my time split across Jira tickets?'. " +
      "Requires the AI tagger to have linked sessions to tickets (ticket_links table).",
    annotations: { title: "Get Task Breakdown", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: {
      type: "object",
      properties: {
        from_time: { type: "string", description: "Exact window start as ISO 8601 UTC, e.g. '2026-05-13T09:00:00Z'. Takes priority over since_hours and date." },
        to_time: { type: "string", description: "Exact window end as ISO 8601 UTC, e.g. '2026-05-13T17:00:00Z'. Used with from_time." },
        date: { type: "string", description: "Date (YYYY-MM-DD). Defaults to today. Ignored if from_time or since_hours is set." },
        since_hours: { type: "number", description: "Look-back window in hours (max 168 = 1 week). Overrides date when set." },
      },
    },
  },
  {
    name: "get-active-task",
    description:
      "Get the Jira ticket currently being worked on, combining the active app session with the AI tagger's classification. " +
      "Checks the tagger's inferred context first, then falls back to the most recently linked session for the same app. " +
      "Use for: 'what Jira ticket am I working on right now?', 'what is my current task?', 'what should I log time against?'. " +
      "Returns the active app, elapsed time, task key, title, status, and confidence.",
    annotations: { title: "Get Active Task", readOnlyHint: true, openWorldHint: false, idempotentHint: true },
    inputSchema: { type: "object", properties: {} },
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

Meridian tracks your app usage by reading screenpipe's ambient recordings and normalising them into structured app sessions. The AI tagger links sessions to Jira tickets and tags each session with activity/tool/topic dimensions.

## Tool Selection

| Question | Tool |
|----------|------|
| "What did I do today?" | get-timeline |
| "How productive was I?" | get-stats |
| "Which apps did I use?" | get-sessions |
| "What am I doing right now?" | get-active-session |
| "What did I do in the last 2 hours?" | get-recent-sessions with hours=2 |
| "What did I work on per Jira ticket today?" | get-task-breakdown |
| "What did I do on KAN-108?" | get-task-sessions with task_key=KAN-108 |
| "What Jira ticket am I working on right now?" | get-active-task |
| "When did I work on X?" | search-sessions with q=X |
| "Full content of a session?" | get-session-detail with id |
| "All-time app usage?" | get-apps |

## Task-linked tools (require the AI tagger to be running)

- **get-active-task** — current app + the Jira ticket the tagger infers you're working on. Falls back to the most recently linked session for the same app. Best for "what should I be logging time to right now?".
- **get-task-sessions** — sessions for a specific Jira ticket key via the tagger's ticket_links classification (more accurate than text search). Supports exact time windows via from_time/to_time or a sliding hour window. Set include_content=true for OCR/audio text. Includes AI dimension tags (activity, tool, topic, intent) per session.
- **get-recent-sessions** — last N hours of sessions with any linked Jira task shown alongside. Best for "what have I been doing lately?".
- **get-task-breakdown** — time per Jira ticket for a date or exact time window, with percentages of total focus time. Best for standups and Jira progress updates. Supports from_time/to_time for precise slot queries.

## Tips

- **Dates** are local calendar dates (YYYY-MM-DD). Today is the default when omitted.
- **from_time / to_time** on get-task-sessions and get-task-breakdown accept ISO 8601 UTC timestamps for exact slot queries (e.g. "2026-05-13T09:00:00Z" to "2026-05-13T13:00:00Z"). Takes priority over since_hours and date.
- **since_hours** on get-task-sessions, get-recent-sessions, and get-task-breakdown uses a sliding window from now — good for "last N hours" without caring about exact clock time.
- **App names** are case-sensitive exact strings — use values from session results (e.g. "code.visualstudio.com").
- **search-sessions** searches window titles, OCR screen text, and audio transcriptions — useful when you don't know the ticket key.
- **get-task-sessions** uses the tagger's ticket_links table (semantic + rule-based) — prefer it over search-sessions when you know the ticket key.
- **get-timeline** includes idle and sleep gaps for a full picture of the day.
- **get-session-detail** returns the full OCR text, audio, and accessibility content for a single session.
- The Meridian daemon runs every 60 seconds — data may be up to 60s stale.
`,
      }],
    };
  }

  throw new Error(`Unknown resource: ${uri}`);
});

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  const spanAttrs: Record<string, string | number | boolean> = { tool_name: name };
  if (args && typeof args === "object") {
    for (const [k, v] of Object.entries(args as Record<string, unknown>)) {
      if (v == null) continue;
      if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") {
        spanAttrs[k] = v;
      }
    }
  }

  return withSpan(`mcp.tool.${name}`, spanAttrs, async (span) => {
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
          SELECT app_name, started_at, last_seen_at, window_titles, frame_count
          FROM active_session WHERE id = 1
        `);

        if (!row) {
          return { content: [{ type: "text", text: "No active session. The Meridian daemon may not have run recently." }] };
        }

        const elapsed = Math.round((Date.now() - new Date(row.last_seen_at as string).getTime()) / 1000);
        const titles = parseJsonColumn<Array<{ window_name: string }>>(row.window_titles as string);
        const topTitle = titles?.[0]?.window_name ?? "";

        const lines = [
          `Active: ${row.app_name}`,
          `Started: ${(row.started_at as string).slice(11, 19)} UTC`,
          `Last seen: ${elapsed}s ago`,
          `Frames: ${row.frame_count}`,
          topTitle ? `Window: ${topTitle}` : "",
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
          WHERE (window_titles LIKE ? OR audio_snippets LIKE ? OR session_text LIKE ?)
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

      case "get-task-sessions": {
        const taskKey = args?.task_key as string;
        if (!taskKey) {
          return { content: [{ type: "text", text: "Error: task_key is required" }] };
        }
        const tw = buildTimeWindow({
          fromTime: args?.from_time as string | undefined,
          toTime: args?.to_time as string | undefined,
          sinceHours: args?.since_hours as number | undefined,
          date: args?.date as string | undefined,
        });
        const includeContent = (args?.include_content as boolean) ?? false;

        const taskMeta = dbGet(db, `
          SELECT title, url, status FROM pm_tasks WHERE task_key = ?
        `, [taskKey]) as { title: string; url: string; status: string } | undefined;

        const rows = dbAll(db, `
          SELECT
            s.id, s.app_name, s.started_at, s.ended_at, s.duration_s,
            s.window_titles, s.session_text,
            s.task_confidence AS confidence, s.task_method AS method, s.task_session_type AS session_type
          FROM app_sessions s
          WHERE s.task_key = ? AND ${tw.cond}
          ORDER BY s.started_at ASC
        `, [taskKey, ...tw.params]) as Array<{
          id: number; app_name: string; started_at: string; ended_at: string;
          duration_s: number; window_titles: string; session_text: string | null;
          confidence: number; method: string; session_type: string;
        }>;

        if (rows.length === 0) {
          return {
            content: [{
              type: "text",
              text: `No sessions linked to ${taskKey} in ${tw.label}.\n` +
                "(The tagger may not have run yet, or no sessions were matched to this ticket.)",
            }],
          };
        }

        // Fetch all dimension tags for these sessions in one query
        const dimRows = dbAll(db, `
          SELECT sd.session_id, sd.dimension, sd.value, sd.confidence
          FROM session_dimensions sd
          WHERE sd.session_id IN (
            SELECT id FROM app_sessions
            WHERE task_key = ? AND ${tw.cond}
          )
          ORDER BY sd.session_id, sd.confidence DESC
        `, [taskKey, ...tw.params] as SqlVal[]) as Array<{
          session_id: number; dimension: string; value: string; confidence: number;
        }>;

        const dimsBySession = new Map<number, Map<string, string[]>>();
        for (const d of dimRows) {
          if (!dimsBySession.has(d.session_id)) dimsBySession.set(d.session_id, new Map());
          const byDim = dimsBySession.get(d.session_id)!;
          if (!byDim.has(d.dimension)) byDim.set(d.dimension, []);
          byDim.get(d.dimension)!.push(d.value);
        }

        const totalS = rows.reduce((sum, r) => sum + r.duration_s, 0);

        const lines: string[] = [
          `${taskKey}${taskMeta?.title ? ` — ${taskMeta.title}` : ""}`,
          ...(taskMeta?.status ? [`Status: ${taskMeta.status}`] : []),
          ...(taskMeta?.url ? [`URL: ${taskMeta.url}`] : []),
          "",
          `Sessions in ${tw.label}: ${rows.length} session${rows.length === 1 ? "" : "s"}, ${fmtDuration(totalS)} total`,
          "",
        ];

        for (const r of rows) {
          const durMin = Math.round(r.duration_s / 60);
          const tStart = r.started_at.slice(11, 16);
          const tEnd = r.ended_at.slice(11, 16);
          const conf = r.confidence != null ? ` · confidence: ${(r.confidence as number).toFixed(2)}` : "";
          const method = r.method ? ` · ${r.method}` : "";

          lines.push(`[#${r.id}] ${r.app_name} | ${tStart}–${tEnd} UTC (${durMin}min)${conf}${method}`);

          const titles = parseJsonColumn<Array<{ window_name: string; count: number }>>(r.window_titles);
          if (titles?.length) {
            const top = titles.slice(0, 5).map(t => `${t.window_name} ×${t.count}`).join(", ");
            lines.push(`  Windows: ${top}`);
          }

          const byDim = dimsBySession.get(r.id);
          if (byDim && byDim.size > 0) {
            const dimStr = Array.from(byDim.entries())
              .map(([dim, vals]) => `${dim}: ${vals.join(", ")}`)
              .join(" · ");
            lines.push(`  Tags: ${dimStr}`);
          }

          if (includeContent && r.session_text) {
            const excerpt = r.session_text.slice(0, 1500).replace(/\n/g, "\n  ");
            lines.push(`  Content: ${excerpt}`);
            if (r.session_text.length > 1500) {
              lines.push(`  ... (${r.session_text.length - 1500} more chars)`);
            }
          }

          lines.push("");
        }

        return { content: [{ type: "text", text: lines.join("\n").trimEnd() }] };
      }

      case "get-recent-sessions": {
        const hours = Math.min(48, Math.max(0.5, (args?.hours as number) ?? 4));
        const appFilter = args?.app as string | undefined;
        const limit = Math.min(50, (args?.limit as number) ?? 20);
        const sinceTs = new Date(Date.now() - hours * 3600 * 1000).toISOString();

        const appCond = appFilter ? "AND s.app_name = ?" : "";
        const params: SqlVal[] = [sinceTs];
        if (appFilter) params.push(appFilter);
        params.push(limit);

        const rows = dbAll(db, `
          SELECT
            s.id, s.app_name, s.started_at, s.ended_at, s.duration_s, s.window_titles,
            s.task_key, s.task_session_type AS session_type,
            pt.title AS task_title
          FROM app_sessions s
          LEFT JOIN pm_tasks pt ON s.task_key = pt.task_key
          WHERE s.started_at >= ? ${appCond}
          ORDER BY s.started_at DESC
          LIMIT ?
        `, params) as Array<{
          id: number; app_name: string; started_at: string; ended_at: string;
          duration_s: number; window_titles: string;
          task_key: string | null; session_type: string | null; task_title: string | null;
        }>;

        if (rows.length === 0) {
          return { content: [{ type: "text", text: `No sessions in the last ${hours}h.` }] };
        }

        const totalS = rows.reduce((sum, r) => sum + r.duration_s, 0);
        const lines: string[] = [
          `Recent sessions (last ${hours}h)${appFilter ? ` · ${appFilter}` : ""}: ${rows.length} session${rows.length === 1 ? "" : "s"}, ${fmtDuration(totalS)} total`,
          "",
        ];

        for (const r of rows) {
          const durMin = Math.round(r.duration_s / 60);
          const tStart = r.started_at.slice(11, 16);
          const tEnd = r.ended_at.slice(11, 16);

          let taskTag = "";
          if (r.task_key && r.session_type !== "overhead") {
            const title = r.task_title ? `: ${r.task_title.slice(0, 60)}` : "";
            taskTag = ` → ${r.task_key}${title}`;
          } else if (r.session_type === "overhead") {
            taskTag = " → (overhead)";
          }

          lines.push(`${tStart}–${tEnd} UTC · ${r.app_name} (${durMin}min)${taskTag}`);

          const titles = parseJsonColumn<Array<{ window_name: string; count: number }>>(r.window_titles);
          if (titles?.length) {
            const top = titles.slice(0, 3).map(t => t.window_name).filter(Boolean).join(", ");
            if (top) lines.push(`  Windows: ${top}`);
          }

          lines.push("");
        }

        return { content: [{ type: "text", text: lines.join("\n").trimEnd() }] };
      }

      case "get-task-breakdown": {
        const tw = buildTimeWindow({
          fromTime: args?.from_time as string | undefined,
          toTime: args?.to_time as string | undefined,
          sinceHours: args?.since_hours as number | undefined,
          date: args?.date as string | undefined,
        });

        const taskRows = dbAll(db, `
          SELECT
            s.task_key,
            pt.title,
            pt.url,
            pt.status,
            SUM(s.duration_s) AS total_s,
            COUNT(*) AS session_count
          FROM app_sessions s
          LEFT JOIN pm_tasks pt ON s.task_key = pt.task_key
          WHERE s.task_key IS NOT NULL AND s.task_session_type != 'overhead' AND ${tw.cond}
          GROUP BY s.task_key, pt.title, pt.url, pt.status
          ORDER BY total_s DESC
        `, tw.params) as Array<{
          task_key: string; title: string | null; url: string | null;
          status: string | null; total_s: number; session_count: number;
        }>;

        const overheadRow = dbGet(db, `
          SELECT COALESCE(SUM(duration_s), 0) AS s, COUNT(*) AS n
          FROM app_sessions
          WHERE (task_key IS NULL OR task_session_type = 'overhead') AND ${tw.cond}
        `, tw.params) as { s: number; n: number } | undefined;

        const focusRow = dbGet(db, `
          SELECT COALESCE(SUM(duration_s), 0) AS s FROM app_sessions WHERE ${tw.plainCond}
        `, tw.params) as { s: number } | undefined;

        const totalS = focusRow?.s ?? 0;

        const lines: string[] = [
          `Task breakdown — ${tw.label}`,
          `Total focus time: ${fmtDuration(totalS)}`,
          "",
        ];

        if (taskRows.length === 0 && (overheadRow?.s ?? 0) === 0) {
          lines.push("No sessions found for this period.");
          return { content: [{ type: "text", text: lines.join("\n") }] };
        }

        if (taskRows.length === 0) {
          lines.push("No sessions linked to Jira tickets yet.");
          lines.push("(Run the tagger daemon to link sessions to tickets.)");
          lines.push("");
        }

        for (const r of taskRows) {
          const pct = totalS > 0 ? Math.round((r.total_s / totalS) * 100) : 0;
          lines.push(`${r.task_key}${r.title ? ` — ${r.title}` : ""}${r.status ? ` (${r.status})` : ""}`);
          lines.push(`  ${fmtDuration(r.total_s)} · ${r.session_count} session${r.session_count === 1 ? "" : "s"} · ${pct}% of focus time`);
          if (r.url) lines.push(`  ${r.url}`);
          lines.push("");
        }

        const ovS = overheadRow?.s ?? 0;
        const ovN = overheadRow?.n ?? 0;
        if (ovS > 0) {
          const ovPct = totalS > 0 ? Math.round((ovS / totalS) * 100) : 0;
          lines.push(`Overhead / untagged: ${fmtDuration(ovS)} · ${ovN} session${ovN === 1 ? "" : "s"} · ${ovPct}% of focus time`);
        }

        return { content: [{ type: "text", text: lines.join("\n").trimEnd() }] };
      }

      case "get-active-task": {
        const active = dbGet(db, `
          SELECT app_name, started_at, last_seen_at, window_titles, frame_count
          FROM active_session WHERE id = 1
        `) as { app_name: string; started_at: string; last_seen_at: string; window_titles: string; frame_count: number } | undefined;

        if (!active) {
          return { content: [{ type: "text", text: "No active session. The Meridian daemon may not have run recently." }] };
        }

        // 1. Check tagger's inferred current task (written to activity_context by the tagger daemon)
        const ctx = dbGet(db, `
          SELECT jira_key, inferred_task, confidence, updated_at
          FROM activity_context WHERE id = 1
        `) as { jira_key: string | null; inferred_task: string | null; confidence: number; updated_at: string } | undefined;

        // 2. Fallback: most recently linked completed session for the same app
        const recent = dbGet(db, `
          SELECT s.task_key, s.task_confidence AS confidence, s.task_method AS method,
                 pt.title, pt.url, pt.status,
                 s.ended_at
          FROM app_sessions s
          LEFT JOIN pm_tasks pt ON s.task_key = pt.task_key
          WHERE s.app_name = ? AND s.task_key IS NOT NULL AND s.task_session_type != 'overhead'
          ORDER BY s.ended_at DESC
          LIMIT 1
        `, [active.app_name]) as {
          task_key: string; confidence: number; method: string;
          title: string | null; url: string | null; status: string | null; ended_at: string;
        } | undefined;

        const elapsed = Math.round((Date.now() - new Date(active.last_seen_at).getTime()) / 1000);
        const durationSoFar = Math.round((Date.now() - new Date(active.started_at).getTime()) / 1000 / 60);
        const titles = parseJsonColumn<Array<{ window_name: string }>>(active.window_titles);
        const topTitle = titles?.[0]?.window_name ?? "";

        const lines: string[] = [
          `Active: ${active.app_name} (${durationSoFar}min so far, last seen ${elapsed}s ago)`,
          topTitle ? `Window: ${topTitle}` : "",
          "",
        ].filter(l => l !== "");

        if (elapsed > 120) {
          lines.push(`⚠ Session data is ${Math.round(elapsed / 60)}min old — Meridian daemon may not be running`);
        }

        const taskKey = ctx?.jira_key ?? recent?.task_key ?? null;

        if (taskKey) {
          const meta = dbGet(db, `
            SELECT title, url, status FROM pm_tasks WHERE task_key = ?
          `, [taskKey]) as { title: string; url: string; status: string } | undefined;

          const fromCtx = !!ctx?.jira_key;
          const confidence = fromCtx ? ctx!.confidence : recent?.confidence;
          const title = meta?.title ?? (fromCtx ? ctx?.inferred_task : recent?.title) ?? null;

          lines.push(`Current task: ${taskKey}${title ? ` — ${title}` : ""}`);
          if (meta?.status ?? recent?.status) lines.push(`Status: ${meta?.status ?? recent?.status}`);
          if (meta?.url ?? recent?.url) lines.push(`URL: ${meta?.url ?? recent?.url}`);
          if (confidence != null) lines.push(`Confidence: ${confidence.toFixed(2)}`);
          if (!fromCtx && recent) {
            lines.push(`(Inferred from last completed ${active.app_name} session at ${recent.ended_at.slice(11, 16)} UTC — active session not yet classified)`);
          } else if (fromCtx && ctx?.updated_at) {
            lines.push(`(Inferred by tagger at ${ctx.updated_at.slice(11, 16)} UTC)`);
          }
        } else {
          lines.push("No task linked yet.");
          lines.push("(Active sessions are classified after they close; the tagger may not have run yet.)");
        }

        return { content: [{ type: "text", text: lines.join("\n") }] };
      }

      case "get-session-detail": {
        const id = args?.id as number;
        if (!id) return { content: [{ type: "text", text: "Error: id is required" }] };
        span.setAttribute("session_id", id);

        const row = dbGet(db, `
          SELECT id, app_name, started_at, ended_at, duration_s,
                 window_titles, signals,
                 frame_count, idle_frame_count, etl_run_id,
                 session_text
          FROM app_sessions WHERE id = ?
        `, [id]);

        if (!row) {
          return { content: [{ type: "text", text: `Session #${id} not found.` }] };
        }

        const durMin = Math.round((row.duration_s as number) / 60);
        const titles = parseJsonColumn<Array<{ window_name: string; count: number }>>(row.window_titles as string);
        const signals = parseJsonColumn<Array<{ event_type: string; value: string; timestamp: string }>>(row.signals as string);
        const sessionText = row.session_text as string | null;

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

        if (signals?.length) {
          lines.push(`Signals (${signals.length}):`);
          signals.forEach((s) => lines.push(`  [${s.event_type}] ${s.value?.slice(0, 100)}`));
          lines.push("");
        }

        if (sessionText) {
          lines.push("Screen content:");
          lines.push(sessionText.slice(0, 4000));
          if (sessionText.length > 4000) {
            lines.push(`... (${sessionText.length - 4000} more chars)`);
          }
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
      span.setAttribute("error", true);
      logger.error({ tool_name: name, err: msg }, "mcp tool failed");
      return { content: [{ type: "text", text: `Error: ${msg}` }] };
    } finally {
      db?.close();
    }
  });
});

async function main() {
  if (process.argv[2] === "install") {
    runInstaller();
    return;
  }
  const transport = new StdioServerTransport();
  await server.connect(transport);
  logger.info({ service: "meridian-mcp" }, "MCP server running on stdio");
}

main().catch((err) => {
  logger.fatal({ err: err instanceof Error ? err.message : String(err) }, "fatal error");
  process.exit(1);
});
