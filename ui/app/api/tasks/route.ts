// meridian — normalises screenpipe activity into structured app sessions

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'

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

    // today session + task associations
    const todaySessions = db.prepare(`
      SELECT s.id, s.duration_s, s.category, s.task_key
      FROM app_sessions s
      WHERE s.started_at >= ? AND s.started_at < ?
        AND s.task_session_type = 'task'
        AND s.task_key IS NOT NULL
    `).all(todayStart, todayEnd) as Array<Record<string, unknown>>

    // week sessions
    const weekSessions = db.prepare(`
      SELECT s.duration_s, s.task_key
      FROM app_sessions s
      WHERE s.started_at >= ? AND s.started_at < ?
        AND s.task_session_type = 'task'
        AND s.task_key IS NOT NULL
    `).all(ws, todayEnd) as Array<Record<string, unknown>>

    // build aggregations
    const todayByTask: Record<string, { dur: number; sessions: number; cats: Record<string, number> }> = {}
    todaySessions.forEach(s => {
      const k = s.task_key as string
      if (!todayByTask[k]) todayByTask[k] = { dur: 0, sessions: 0, cats: {} }
      todayByTask[k].dur += s.duration_s as number
      todayByTask[k].sessions++
      const cat = (s.category as string) || 'idle_personal'
      todayByTask[k].cats[cat] = (todayByTask[k].cats[cat] ?? 0) + (s.duration_s as number)
    })

    const weekByTask: Record<string, number> = {}
    weekSessions.forEach(s => {
      const k = s.task_key as string
      weekByTask[k] = (weekByTask[k] ?? 0) + (s.duration_s as number)
    })

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
