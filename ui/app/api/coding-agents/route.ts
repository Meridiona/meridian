// meridian — normalises screenpipe activity into structured app sessions

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { todayString } from '@/lib/date-utils'

export const dynamic = 'force-dynamic'

interface AgentTotal {
  app: string
  total_s: number
}

export interface CodingAgentsResponse {
  date: string
  total_s: number          // union across ALL coding-agent sessions (overlap deduped)
  agents: AgentTotal[]      // per-agent union, descending
}

const CODING_AGENTS = ['Claude Code', 'Codex']

/** Union of [start,end] wall intervals, in seconds — dedups parallel overlap. */
function unionSeconds(rows: Array<{ started_at: string; ended_at: string }>): number {
  const ivs = rows
    .map(r => [new Date(r.started_at).getTime(), new Date(r.ended_at).getTime()] as [number, number])
    .filter(([s, e]) => e > s)
    .sort((a, b) => a[0] - b[0])
  let total = 0
  let curStart = -1
  let curEnd = -1
  for (const [s, e] of ivs) {
    if (s > curEnd) {
      if (curEnd > curStart) total += curEnd - curStart
      curStart = s
      curEnd = e
    } else if (e > curEnd) {
      curEnd = e
    }
  }
  if (curEnd > curStart) total += curEnd - curStart
  return Math.round(total / 1000)
}

export async function GET() {
  const date = todayString()
  try {
    const db = getDb()
    // Coding-agent rows carry claude_session_uuid and a local-day bucket.
    const rows = db.prepare(`
      SELECT app_name, started_at, ended_at
      FROM app_sessions
      WHERE claude_session_uuid IS NOT NULL
        AND substr(started_at, 1, 10) = ?
        AND app_name IN ('Claude Code', 'Codex')
    `).all(date) as Array<{ app_name: string; started_at: string; ended_at: string }>

    const agents: AgentTotal[] = CODING_AGENTS
      .map(app => ({
        app,
        total_s: unionSeconds(rows.filter(r => r.app_name === app)),
      }))
      .filter(a => a.total_s > 0)
      .sort((a, b) => b.total_s - a.total_s)

    return NextResponse.json({
      date,
      total_s: unionSeconds(rows),
      agents,
    } satisfies CodingAgentsResponse)
  } catch (e) {
    console.error('coding-agents api error:', e)
    return NextResponse.json({ date, total_s: 0, agents: [] } satisfies CodingAgentsResponse)
  }
}
