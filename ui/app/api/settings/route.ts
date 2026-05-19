// meridian — normalises screenpipe activity into structured app sessions
import { NextResponse } from 'next/server'
import { readSettings, writeSettings } from '@/lib/settings'

export const dynamic = 'force-dynamic'

export async function GET() {
  const settings = readSettings()
  return NextResponse.json(settings)
}

export async function PUT(req: Request) {
  const body = await req.json()
  const current = readSettings()
  const updated = { ...current, ...body }
  writeSettings(updated)
  return NextResponse.json(updated)
}
