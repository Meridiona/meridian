// meridian — normalises screenpipe activity into structured app sessions

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'
import { sessionInterval, unionSeconds } from '@/lib/intervals'

export const dynamic = 'force-dynamic'

export interface TaskSummary {
  key: string
  title: string
  description: string
  status: string
  provider: string
  url: string
  today_s: number
  week_s: number
  session_count: number
  cats: Record<string, number>
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
      SELECT task_key, title, description_text, status_category, provider, url
      FROM pm_tasks
      ORDER BY task_key DESC
    `).all() as Array<Record<string, unknown>>

    // Per-task time is the UNION of every session linked to the task, across
    // BOTH recording streams — foreground screen capture AND the coding-agent
    // transcript overlay. The two streams record the same wall-clock from two
    // angles, so SUMMING duration_s double-counts every overlapping second
    // (and lets a parked-open agent window inflate a task to impossible hours).
    // `sessionInterval` caps agent rows to their engaged duration; `unionSeconds`
    // then counts overlapping time once and agent-only time still counts.
    interface SessionRow { started_at: string; ended_at: string; duration_s: number; claude_session_uuid: string | null; category: string | null; task_key: string }

    const todaySessions = db.prepare(`
      SELECT s.started_at, s.ended_at, s.duration_s, s.claude_session_uuid, s.category, s.task_key
      FROM app_sessions s
      WHERE s.started_at >= ? AND s.started_at < ?
        AND s.task_session_type = 'task'
        AND s.task_key IS NOT NULL
    `).all(todayStart, todayEnd) as SessionRow[]

    const weekSessions = db.prepare(`
      SELECT s.started_at, s.ended_at, s.duration_s, s.claude_session_uuid, s.task_key
      FROM app_sessions s
      WHERE s.started_at >= ? AND s.started_at < ?
        AND s.task_session_type = 'task'
        AND s.task_key IS NOT NULL
    `).all(ws, todayEnd) as SessionRow[]

    // group rows by task, then union each group's intervals
    const todayRowsByTask: Record<string, SessionRow[]> = {}
    todaySessions.forEach(s => { (todayRowsByTask[s.task_key] ??= []).push(s) })
    const weekRowsByTask: Record<string, SessionRow[]> = {}
    weekSessions.forEach(s => { (weekRowsByTask[s.task_key] ??= []).push(s) })

    const todayByTask: Record<string, { dur: number; sessions: number; cats: Record<string, number> }> = {}
    for (const [k, rows] of Object.entries(todayRowsByTask)) {
      // Category split is the FOREGROUND share only — the agent overlay is the
      // same work from a second angle, so folding its raw duration in here would
      // re-introduce the double-count the union exists to prevent.
      const cats: Record<string, number> = {}
      rows.filter(r => r.claude_session_uuid == null).forEach(r => {
        const cat = r.category || 'idle_personal'
        cats[cat] = (cats[cat] ?? 0) + r.duration_s
      })
      todayByTask[k] = {
        dur: unionSeconds(rows.map(sessionInterval)),
        sessions: rows.length,
        cats,
      }
    }

    const weekByTask: Record<string, number> = {}
    for (const [k, rows] of Object.entries(weekRowsByTask)) {
      weekByTask[k] = unionSeconds(rows.map(sessionInterval))
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
        status: (t.status_category as string) || 'todo',
        provider: (t.provider as string) || 'jira',
        url: (t.url as string) || '',
        today_s: agg?.dur ?? 0,
        week_s: weekByTask[k] ?? 0,
        session_count: agg?.sessions ?? 0,
        cats: agg?.cats ?? {},
      }
    }).sort((a, b) => b.today_s - a.today_s)

    return NextResponse.json({ tasks, unassigned_s: unassigned?.total ?? 0 })
  } catch (e) {
    console.error('tasks api error:', e)
    return NextResponse.json({ tasks: [], unassigned_s: 0 })
  }
}
