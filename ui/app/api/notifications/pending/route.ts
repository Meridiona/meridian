//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GET /api/notifications/pending — native-channel notifications ready to fire.
// The tray relay polls this, delivers each as a macOS toast, then POSTs to
// /api/notifications/:id/delivered. Preference + quiet-hours filtering happens
// server-side (in lib/notifications) so the tray stays a dumb delivery agent.

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { pendingNative } from '@/lib/notifications'

export const dynamic = 'force-dynamic'

export async function GET() {
  try {
    const rows = pendingNative(getDb())
    return NextResponse.json(
      rows.map(r => ({ id: r.id, title: r.title, body: r.body, deep_link: r.deep_link, severity: r.severity })),
    )
  } catch {
    // Pre-migration-042 DB (no notifications table) or transient read error —
    // return an empty queue rather than erroring the tray's poll loop.
    return NextResponse.json([])
  }
}
