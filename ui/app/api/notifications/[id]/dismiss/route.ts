//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// POST /api/notifications/:id/dismiss — the dashboard calls this when the user
// dismisses an in-app notification banner. Idempotent.

import { NextResponse } from 'next/server'
import { getWriteDb } from '@/lib/db-write'
import { dismissBanner } from '@/lib/notifications'
import { refresh } from '@/lib/notifications-banner-store'

export const dynamic = 'force-dynamic'

export async function POST(_req: Request, { params }: { params: Promise<{ id: string }> }) {
  const { id } = await params
  const nid = Number(id)
  if (!Number.isInteger(nid) || nid <= 0) {
    return NextResponse.json({ error: 'bad id' }, { status: 400 })
  }
  try {
    dismissBanner(getWriteDb(), nid)
    refresh() // push the updated banner set to open SSE connections immediately
    return NextResponse.json({ ok: true })
  } catch (e) {
    console.error('notification dismiss error:', e)
    return NextResponse.json({ error: 'write failed' }, { status: 500 })
  }
}
