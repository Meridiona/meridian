//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Returns the last N log entries from the daemon's JSONL file. Used to
// populate the initial log view before the SSE tail takes over.

import { NextResponse } from 'next/server'
import { readRecentLines } from '@/lib/log-tail'

export const dynamic = 'force-dynamic'

export async function GET(request: Request) {
  const { searchParams } = new URL(request.url)
  const limit = Math.min(parseInt(searchParams.get('limit') ?? '200', 10), 1000)
  const entries = await readRecentLines(limit)
  return NextResponse.json(entries)
}
