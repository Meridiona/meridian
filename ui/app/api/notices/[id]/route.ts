//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// DELETE /api/notices/:id — clear a notice from system_notices immediately.
// Called by the UI when a provider successfully connects so the error banner
// disappears without waiting for the next ETL poll cycle.

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { refresh } from '@/lib/notices-store'

export const dynamic = 'force-dynamic'

export async function DELETE(
  _request: Request,
  { params }: { params: { id: string } },
) {
  const noticeId = params.id
  if (!noticeId) {
    return NextResponse.json({ error: 'Missing notice id' }, { status: 400 })
  }
  try {
    const db = getDb()
    db.prepare('DELETE FROM system_notices WHERE notice_id = ?').run(noticeId)
    refresh() // push empty/updated list to all open SSE connections immediately
    return NextResponse.json({ ok: true })
  } catch (e) {
    return NextResponse.json({ error: String(e) }, { status: 500 })
  }
}
