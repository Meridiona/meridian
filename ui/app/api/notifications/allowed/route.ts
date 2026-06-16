//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GET /api/notifications/allowed?event=<event_key> — whether a notification for
// this event may fire RIGHT NOW under the user's preferences (master switch +
// per-type toggle + quiet hours). The tray calls this before firing its direct
// health/pause toasts, which don't flow through the outbox (the daemon can't
// enqueue "I'm down" while it's down). Keeps the policy in one place server-side.

import { NextResponse } from 'next/server'
import { readSettings } from '@/lib/settings'
import { eventAllowed, inQuietHours } from '@/lib/notifications'

export const dynamic = 'force-dynamic'

export async function GET(req: Request) {
  const event = new URL(req.url).searchParams.get('event') ?? ''
  const s = readSettings()
  // Quiet hours gate toasts (the only channel for these direct events).
  const allowed = eventAllowed(event, s) && !inQuietHours(s)
  return NextResponse.json({ allowed })
}
