//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GET /api/plan/task?key=KAN-123 — full detail for one board ticket, for the
// plan page's task dialog. Returns the FULL description + acceptance criteria
// (the list payload only carries a short excerpt), plus all display meta.

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { dueDaysFrom } from '@/lib/daily-plan'

export const dynamic = 'force-dynamic'

export interface TaskDetail {
  key: string
  title: string
  provider: string
  url: string
  status: string
  is_terminal: boolean
  issue_type: string
  epic: string | null
  priority: string | null
  story_points: string | null
  due_date: string | null
  due_days: number | null
  start_date: string | null
  description: string
  acceptance_criteria: string | null
}

export async function GET(req: Request) {
  try {
    const key = new URL(req.url).searchParams.get('key')
    if (!key) return NextResponse.json({ error: 'key required' }, { status: 400 })

    const db = getDb()
    const r = db.prepare(`
      SELECT task_key, title, provider, url,
             COALESCE(status_raw,'') AS status_raw, COALESCE(is_terminal,0) AS is_terminal,
             COALESCE(issue_type,'') AS issue_type, epic_title, parent_key,
             priority, story_points, due_date, start_date,
             COALESCE(description_text,'') AS description_text, acceptance_criteria
      FROM pm_tasks WHERE task_key = ?
    `).get(key) as Record<string, unknown> | undefined

    if (!r) return NextResponse.json({ error: 'not found' }, { status: 404 })

    const detail: TaskDetail = {
      key: r.task_key as string,
      title: (r.title as string) ?? (r.task_key as string),
      provider: (r.provider as string) || 'jira',
      url: (r.url as string) || '',
      status: (r.status_raw as string) || '',
      is_terminal: !!(r.is_terminal as number),
      issue_type: (r.issue_type as string) || '',
      epic: ((r.epic_title as string)?.trim() || (r.parent_key as string)?.trim() || null) ?? null,
      priority: (r.priority as string)?.trim() || null,
      story_points: (r.story_points as string)?.trim() || null,
      due_date: (r.due_date as string | null) ?? null,
      due_days: dueDaysFrom((r.due_date as string | null) ?? null, new Date()),
      start_date: (r.start_date as string | null) ?? null,
      description: (r.description_text as string) || '',
      acceptance_criteria: (r.acceptance_criteria as string)?.trim() || null,
    }
    return NextResponse.json(detail)
  } catch (e) {
    console.error('plan task detail error:', e)
    return NextResponse.json({ error: 'failed' }, { status: 500 })
  }
}
