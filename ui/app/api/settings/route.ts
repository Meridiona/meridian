//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { NextResponse } from 'next/server'
import { readSettings, writeSettings } from '@/lib/settings'

export const dynamic = 'force-dynamic'

// Sentinel returned to the browser when a password is stored. The browser
// never receives the real value; the PUT handler recognises this sentinel
// (and the empty string) as "keep existing password".
const PASSWORD_SENTINEL = '••••••••'

export async function GET() {
  const settings = readSettings()
  return NextResponse.json({
    ...settings,
    // Redact stored password — return sentinel if set, empty if not.
    oo_password: settings.oo_password ? PASSWORD_SENTINEL : '',
  })
}

export async function PUT(req: Request) {
  const body = await req.json() as Record<string, unknown>
  const current = readSettings()

  // Validate OTLP endpoint — must be http/https if non-empty.
  const ep = body.otlp_endpoint
  if (typeof ep === 'string' && ep.trim() && !ep.startsWith('http://') && !ep.startsWith('https://')) {
    return NextResponse.json(
      { error: 'otlp_endpoint must start with http:// or https://' },
      { status: 400 },
    )
  }

  // Validate credentials — reject newlines (HTTP header injection vector).
  for (const field of ['oo_email', 'oo_password'] as const) {
    const v = body[field]
    if (typeof v === 'string' && (v.includes('\n') || v.includes('\r'))) {
      return NextResponse.json({ error: `${field} contains invalid characters` }, { status: 400 })
    }
  }

  const updated = { ...current, ...body }

  // If the client sends the sentinel or an empty string for oo_password,
  // they did not change it — preserve whatever is stored on disk.
  const sentPassword = body.oo_password
  if (!sentPassword || sentPassword === PASSWORD_SENTINEL) {
    updated.oo_password = current.oo_password
  }

  writeSettings(updated)
  return NextResponse.json({
    ...updated,
    oo_password: updated.oo_password ? PASSWORD_SENTINEL : '',
  })
}
