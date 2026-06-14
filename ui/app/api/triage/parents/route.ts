//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// GET /api/triage/parents?provider=jira&key=KAN-109
//
// For the "link to a parent" hygiene fix: list valid parents for the ticket (the
// level above it — Epic / parent task / parent work item, per the tracker's
// hierarchy), a label for that level, and a deep link to create a new parent in
// the tracker. Auth lives in the daemon, so the UI spawns `meridian ticket-parents`
// (read-only) and relays its JSON.

import { NextResponse } from 'next/server'
import { spawn } from 'child_process'
import { meridianCandidates, selectMeridianBinary } from '@/lib/meridian-bin'

export const dynamic = 'force-dynamic'

interface ParentsOutput {
  parents: Array<{ key: string; title: string }>
  parent_label: string
  create_url: string
}

function runParents(provider: string, key: string): Promise<{ ok: boolean; out?: ParentsOutput; error?: string }> {
  return new Promise(resolve => {
    const child = spawn(selectMeridianBinary(meridianCandidates()), ['ticket-parents', '--provider', provider, '--key', key], {
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout?.on('data', (d: Buffer) => { stdout += d.toString() })
    child.stderr?.on('data', (d: Buffer) => { stderr += d.toString() })
    child.on('error', err => { clearTimeout(timer); resolve({ ok: false, error: `spawn error: ${err.message}` }) })
    const timer = setTimeout(() => { child.kill(); resolve({ ok: false, error: 'ticket-parents timed out' }) }, 30_000)
    child.on('close', code => {
      clearTimeout(timer)
      if (code !== 0) { resolve({ ok: false, error: stderr.trim() || `exited ${code}` }); return }
      try {
        const line = stdout.trim().split('\n').filter(Boolean).pop() ?? ''
        resolve({ ok: true, out: JSON.parse(line) as ParentsOutput })
      } catch {
        resolve({ ok: false, error: `could not parse: ${stdout.trim().slice(-200)}` })
      }
    })
  })
}

export async function GET(req: Request) {
  const { searchParams } = new URL(req.url)
  const provider = searchParams.get('provider') ?? ''
  const key = searchParams.get('key') ?? ''
  if (!provider || !key) {
    return NextResponse.json({ error: 'provider and key are required' }, { status: 400 })
  }
  const res = await runParents(provider, key)
  if (!res.ok) {
    return NextResponse.json({ parents: [], parent_label: 'parent', create_url: '', error: res.error }, { status: 200 })
  }
  return NextResponse.json(res.out)
}
