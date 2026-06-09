// meridian — normalises screenpipe activity into structured app sessions

import { NextResponse } from 'next/server'
import { spawn } from 'child_process'

export const dynamic = 'force-dynamic'

// Candidate paths for the meridian binary. The shell-script wrapper at
// ~/.local/bin/meridian delegates to the daemon via cmd_daemon_passthrough,
// which is what we need. Launchd's PATH lacks ~/.local/bin, so we probe
// candidate locations rather than relying on $PATH.
const MERIDIAN_CANDIDATES = [
  `${process.env.HOME}/.local/bin/meridian`,
  '/usr/local/bin/meridian',
  `${process.env.HOME}/.meridian/app/bin/meridian`,
]

function runTasksSync(): Promise<{ ok: boolean; stdout: string; stderr: string }> {
  const bin = MERIDIAN_CANDIDATES.find(p => {
    try { require('fs').accessSync(p, require('fs').constants.X_OK); return true } catch { return false }
  }) ?? MERIDIAN_CANDIDATES[0]

  return new Promise(resolve => {
    const child = spawn(bin, ['tasks-sync'], {
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    let stdout = ''
    let stderr = ''
    child.stdout?.on('data', (d: Buffer) => { stdout += d.toString() })
    child.stderr?.on('data', (d: Buffer) => { stderr += d.toString() })

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
