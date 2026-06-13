//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// POST /api/triage/apply  { provider, key, field, value }
//
// Apply ONE board-hygiene fix to the user's real tracker. Auth + the provider
// write live in the daemon, so the UI spawns `meridian ticket-update` (the same
// pattern /api/tasks/sync uses) and relays its JSON result. The result is either
// { status: 'applied' } — the write landed and the local mirror was re-synced —
// or { status: 'redirected', browse_url } — this provider has no API for that
// field, so the dialog opens the ticket in the tracker instead.

import { NextResponse } from 'next/server'
import { spawn } from 'child_process'
import fs from 'fs'

export const dynamic = 'force-dynamic'

const MERIDIAN_CANDIDATES = [
  `${process.env.HOME}/.local/bin/meridian`,
  '/usr/local/bin/meridian',
  `${process.env.HOME}/.meridian/app/bin/meridian`,
]

function meridianBin(): string {
  return (
    MERIDIAN_CANDIDATES.find(p => {
      try {
        fs.accessSync(p, fs.constants.X_OK)
        return true
      } catch {
        return false
      }
    }) ?? MERIDIAN_CANDIDATES[0]
  )
}

interface ApplyOutput {
  status: 'applied' | 'redirected'
  provider: string
  key: string
  field: string
  browse_url?: string
  reason?: string
}

function runUpdate(
  provider: string,
  key: string,
  field: string,
  value: string,
): Promise<{ ok: boolean; out?: ApplyOutput; error?: string }> {
  return new Promise(resolve => {
    const args = ['ticket-update', '--provider', provider, '--key', key, '--field', field, '--value', value]
    const child = spawn(meridianBin(), args, { stdio: ['ignore', 'pipe', 'pipe'] })

    let stdout = ''
    let stderr = ''
    child.stdout?.on('data', (d: Buffer) => { stdout += d.toString() })
    child.stderr?.on('data', (d: Buffer) => { stderr += d.toString() })

    child.on('error', err => {
      clearTimeout(timer)
      resolve({ ok: false, error: `spawn error: ${err.message}` })
    })

    // Writes can re-sync the whole board, so allow generous headroom.
    const timer = setTimeout(() => {
      child.kill()
      resolve({ ok: false, error: 'ticket-update timed out after 60s' })
    }, 60_000)

    child.on('close', code => {
      clearTimeout(timer)
      if (code !== 0) {
        resolve({ ok: false, error: stderr.trim() || `ticket-update exited ${code}` })
        return
      }
      try {
        // The result JSON is the last non-empty stdout line (sync logs precede it).
        const line = stdout.trim().split('\n').filter(Boolean).pop() ?? ''
        resolve({ ok: true, out: JSON.parse(line) as ApplyOutput })
      } catch {
        resolve({ ok: false, error: `could not parse result: ${stdout.trim().slice(-200)}` })
      }
    })
  })
}

export async function POST(req: Request) {
  let body: { provider?: unknown; key?: unknown; field?: unknown; value?: unknown }
  try {
    body = await req.json()
  } catch {
    return NextResponse.json({ error: 'bad json' }, { status: 400 })
  }

  const provider = typeof body.provider === 'string' ? body.provider : ''
  const key = typeof body.key === 'string' ? body.key : ''
  const field = typeof body.field === 'string' ? body.field : ''
  const value = typeof body.value === 'string' ? body.value : ''

  if (!provider || !key || !field) {
    return NextResponse.json({ error: 'provider, key and field are required' }, { status: 400 })
  }

  const res = await runUpdate(provider, key, field, value)
  if (!res.ok) {
    return NextResponse.json({ error: res.error ?? 'apply failed' }, { status: 500 })
  }
  return NextResponse.json({ ok: true, result: res.out })
}
