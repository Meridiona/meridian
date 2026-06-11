//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
