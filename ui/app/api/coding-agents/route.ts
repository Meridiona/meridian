//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { todayString } from '@/lib/date-utils'
import { unionSeconds } from '@/lib/intervals'

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

export async function GET() {
  const date = todayString()
  try {
    const db = getDb()
    // Coding-agent rows carry coding_agent_session_uuid and a local-day bucket.
    const rows = db.prepare(`
      SELECT app_name, started_at, ended_at
      FROM app_sessions
      WHERE coding_agent_session_uuid IS NOT NULL
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
