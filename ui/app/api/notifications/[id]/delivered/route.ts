//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// POST /api/notifications/:id/delivered — the tray calls this once it has shown
// the macOS toast, so the row is never re-delivered. Idempotent.

import { NextResponse } from 'next/server'
import { getWriteDb } from '@/lib/db-write'
import { markNativeDelivered } from '@/lib/notifications'

export const dynamic = 'force-dynamic'

export async function POST(_req: Request, { params }: { params: Promise<{ id: string }> }) {
  const { id } = await params
  const nid = Number(id)
  if (!Number.isInteger(nid) || nid <= 0) {
    return NextResponse.json({ error: 'bad id' }, { status: 400 })
  }
  try {
    markNativeDelivered(getWriteDb(), nid)
    return NextResponse.json({ ok: true })
  } catch (e) {
    console.error('notification delivered error:', e)
    return NextResponse.json({ error: 'write failed' }, { status: 500 })
  }
}
