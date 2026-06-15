//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { NextResponse } from 'next/server'
import { spawn } from 'child_process'
import { meridianCandidates, selectMeridianBinary } from '@/lib/meridian-bin'

export const dynamic = 'force-dynamic'

function runTasksSync(): Promise<{ ok: boolean; stdout: string; stderr: string }> {
  // Prefer the native binary over the node wrapper: launchd's PATH lacks `node`,
  // so probing the wrapper first broke sync on installed dashboards. See
  // lib/meridian-bin.ts for the ordering rationale.
  const bin = selectMeridianBinary(meridianCandidates())

  return new Promise(resolve => {
    const child = spawn(bin, ['tasks-sync'], {
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    let stdout = ''
    let stderr = ''
    child.stdout?.on('data', (d: Buffer) => { stdout += d.toString() })
    child.stderr?.on('data', (d: Buffer) => { stderr += d.toString() })

    child.on('error', (err) => {
      clearTimeout(timer)
      resolve({ ok: false, stdout, stderr: `spawn error: ${err.message}` })
    })

    const timer = setTimeout(() => {
      child.kill()
      resolve({ ok: false, stdout, stderr: stderr + '\ntasks-sync timed out after 30s' })
    }, 30_000)

    child.on('close', code => {
      clearTimeout(timer)
      resolve({ ok: code === 0, stdout, stderr })
    })
  })
}

export async function POST() {
  const { ok, stdout, stderr } = await runTasksSync()
  if (ok) {
    return NextResponse.json({ ok: true, detail: stdout.trim() })
  }
  return NextResponse.json({ ok: false, error: stderr.trim() || 'tasks-sync failed' }, { status: 500 })
}
