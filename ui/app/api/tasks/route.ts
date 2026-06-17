//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'
import { sessionInterval, unionSeconds, intersectSeconds, mergeIntervals, type Interval } from '@/lib/intervals'
import { parseIssues, type Hygiene } from '@/lib/hygiene'

export const dynamic = 'force-dynamic'

export interface TaskSummary {
  key: string
  title: string
  description: string
  issue_type: string
  status: string        // verbatim provider status / column name (may be empty)
  is_terminal: boolean  // whether that status means the ticket is done/closed
  provider: string
  url: string
  epic_key: string | null
  epic_title: string | null
  due_date: string | null
  start_date: string | null
  today_s: number
  today_autonomous_s: number  // agent time on the task that ran while you were away
  week_s: number
  session_count: number
  cats: Record<string, number>
  hygiene: Hygiene | null  // board-hygiene flags + fixes (null until triaged)
}

export interface TasksResponse {
  tasks: TaskSummary[]
  unassigned_s: number
}

export async function GET() {
  const today = todayString()
  const { start: todayStart, end: todayEnd } = localDayBounds(today)

  // 7-day window for week totals
  const weekStart = new Date(Date.now() - 6 * 86400000).toLocaleDateString('en-CA')
  const { start: ws } = localDayBounds(weekStart)

  try {
    const db = getDb()

    // get all tasks
    const taskRows = db.prepare(`
      SELECT task_key, title, description_text, COALESCE(issue_type,'') AS issue_type, status_raw, is_terminal, provider, url, parent_key, epic_title, due_date, start_date
      FROM pm_tasks
      ORDER BY task_key DESC
    `).all() as Array<Record<string, unknown>>

    // Board-hygiene verdicts the daemon wrote into pm_task_curation (after each
    // sync). Tolerate a DB that predates migration 038 (table absent).
    const hygieneByKey: Record<string, Hygiene> = {}
    const hasCuration = db.prepare(
      "SELECT 1 FROM sqlite_master WHERE type='table' AND name='pm_task_curation'",
    ).get()
    if (hasCuration) {
      // ignored_codes arrived in migration 040 — tolerate older DBs.
      const hasIgnored = db.prepare(
        "SELECT 1 FROM pragma_table_info('pm_task_curation') WHERE name='ignored_codes'",
      ).get()
      const cols = hasIgnored
        ? 'task_key, bucket, reasons_json, decision, snoozed_until, ignored_codes'
        : "task_key, bucket, reasons_json, decision, snoozed_until, '[]' AS ignored_codes"
      const cur = db.prepare(`SELECT ${cols} FROM pm_task_curation`)
        .all() as Array<{ task_key: string; bucket: string; reasons_json: string; decision: string | null; snoozed_until: string | null; ignored_codes: string }>
      const nowIso = new Date().toISOString()
      for (const c of cur) {
        // Snoozed-until-future tickets drop off (no hygiene) until the snooze lapses —
        // this is what makes the cleanup page's "Later" actually defer the ticket.
        if (c.snoozed_until && c.snoozed_until > nowIso) continue
        hygieneByKey[c.task_key] = {
          bucket: c.bucket,
          issues: parseIssues(c.reasons_json, c.ignored_codes),
          decision: c.decision,
        }
      }
    }

    // A task's time = YOUR hands-on time on it + the agent time that ran while
    // you were AWAY (autonomous). Agent time alongside you (supervised) is not
    // added — that wall-clock is already your presence, and adding it would
    // double-count the day and inflate a task past your Focus. So we need your
    // full presence (every foreground session, task or not) to tell autonomous
    // from supervised agent time.
    interface SessionRow { started_at: string; ended_at: string; duration_s: number; coding_agent_session_uuid: string | null; category: string | null; task_key: string }

    const fgPresenceRows = (start: string, end: string) =>
      (db.prepare(`
        SELECT s.started_at, s.ended_at, s.duration_s, s.coding_agent_session_uuid, s.category, s.task_key
        FROM app_sessions s
        WHERE s.started_at >= ? AND s.started_at < ? AND s.coding_agent_session_uuid IS NULL
      `).all(start, end) as SessionRow[])
        .map(r => ({ started_at: r.started_at, ended_at: r.ended_at }))

    const todayPresence = mergeIntervals(fgPresenceRows(todayStart, todayEnd))
    const weekPresence = mergeIntervals(fgPresenceRows(ws, todayEnd))

    const taskSessions = (start: string, end: string) =>
      db.prepare(`
        SELECT s.started_at, s.ended_at, s.duration_s, s.coding_agent_session_uuid, s.category, s.task_key
        FROM app_sessions s
        WHERE s.started_at >= ? AND s.started_at < ?
          AND s.task_session_type = 'task'
          AND s.task_key IS NOT NULL
      `).all(start, end) as SessionRow[]

    const todaySessions = taskSessions(todayStart, todayEnd)
    const weekSessions = taskSessions(ws, todayEnd)

    // your time on the task + autonomous agent time (agent intervals outside presence)
    const taskTime = (rows: SessionRow[], presence: Interval[]) => {
      const fg = rows.filter(r => r.coding_agent_session_uuid == null).map(sessionInterval)
      const agent = rows.filter(r => r.coding_agent_session_uuid != null).map(sessionInterval)
      const your_s = unionSeconds(fg)
      const autonomous_s = Math.max(0, unionSeconds(agent) - intersectSeconds(agent, presence))
      return { your_s, autonomous_s, total_s: your_s + autonomous_s }
    }

    const todayRowsByTask: Record<string, SessionRow[]> = {}
    todaySessions.forEach(s => { (todayRowsByTask[s.task_key] ??= []).push(s) })
    const weekRowsByTask: Record<string, SessionRow[]> = {}
    weekSessions.forEach(s => { (weekRowsByTask[s.task_key] ??= []).push(s) })

    const todayByTask: Record<string, { dur: number; autonomous_s: number; sessions: number; cats: Record<string, number> }> = {}
    for (const [k, rows] of Object.entries(todayRowsByTask)) {
      // Category split is the FOREGROUND share only — proportions for the bar.
      const fgRows = rows.filter(r => r.coding_agent_session_uuid == null)
      const cats: Record<string, number> = {}
      fgRows.forEach(r => {
        const cat = r.category || 'idle_personal'
        cats[cat] = (cats[cat] ?? 0) + r.duration_s
      })
      const t = taskTime(rows, todayPresence)
      todayByTask[k] = { dur: t.total_s, autonomous_s: t.autonomous_s, sessions: fgRows.length, cats }
    }

    const weekByTask: Record<string, number> = {}
    for (const [k, rows] of Object.entries(weekRowsByTask)) {
      weekByTask[k] = taskTime(rows, weekPresence).total_s
    }

    // unassigned today
    const unassigned = db.prepare(`
      SELECT COALESCE(SUM(s.duration_s), 0) as total
      FROM app_sessions s
      WHERE s.started_at >= ? AND s.started_at < ?
        AND (s.task_method IS NULL OR s.task_session_type = 'overhead')
    `).get(todayStart, todayEnd) as { total: number }

    const tasks: TaskSummary[] = taskRows.map(t => {
      const k = t.task_key as string
      const agg = todayByTask[k]
      return {
        key: k,
        title: t.title as string,
        description: (t.description_text as string) || '',
        issue_type: (t.issue_type as string) || '',
        status: (t.status_raw as string) || '',
        is_terminal: !!(t.is_terminal as number),
        provider: (t.provider as string) || 'jira',
        url: (t.url as string) || '',
        epic_key: (t.parent_key as string | null) ?? null,
        epic_title: (t.epic_title as string | null) ?? null,
        due_date: (t.due_date as string | null) ?? null,
        start_date: (t.start_date as string | null) ?? null,
        today_s: agg?.dur ?? 0,
        today_autonomous_s: agg?.autonomous_s ?? 0,
        week_s: weekByTask[k] ?? 0,
        session_count: agg?.sessions ?? 0,
        cats: agg?.cats ?? {},
        hygiene: hygieneByKey[k] ?? null,
      }
    }).sort((a, b) => b.today_s - a.today_s)

    return NextResponse.json({ tasks, unassigned_s: unassigned?.total ?? 0 })
  } catch (e) {
    console.error('tasks api error:', e)
    return NextResponse.json({ tasks: [], unassigned_s: 0 })
  }
}
