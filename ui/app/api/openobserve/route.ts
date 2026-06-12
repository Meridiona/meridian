//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// POST /api/openobserve — start or stop the local OpenObserve launchd service
// (com.meridiona.openobserve) to match the "OpenObserve Export" toggle.
//
// The toggle gates the SERVICE, not just the daemon's exporters: with export
// disabled there is no idle OpenObserve server holding :5080 and its memory
// caps. Disable also persists across logins (launchctl disable), so RunAtLoad
// cannot resurrect the service after a reboot.

import { execFile } from 'child_process'
import { promisify } from 'util'
import fs from 'fs'
import os from 'os'
import path from 'path'

export const dynamic = 'force-dynamic'

const execFileP = promisify(execFile)
const LABEL = 'com.meridiona.openobserve'

async function launchctl(...args: string[]): Promise<{ ok: boolean; err?: string }> {
  try {
    await execFileP('launchctl', args)
    return { ok: true }
  } catch (e) {
    return { ok: false, err: e instanceof Error ? e.message : String(e) }
  }
}

export async function POST(req: Request) {
  const body = await req.json() as { enabled?: unknown }
  if (typeof body.enabled !== 'boolean') {
    return Response.json({ error: 'enabled must be a boolean' }, { status: 400 })
  }
  const uid = process.getuid?.()
  if (uid === undefined) {
    return Response.json({ error: 'cannot determine uid' }, { status: 500 })
  }
  const domain = `gui/${uid}`
  const target = `${domain}/${LABEL}`
  const plist = path.join(/*turbopackIgnore: true*/ os.homedir(), 'Library', 'LaunchAgents', `${LABEL}.plist`)

  if (body.enabled) {
    if (!fs.existsSync(/*turbopackIgnore: true*/ plist)) {
      return Response.json(
        { error: 'OpenObserve agent not installed — run scripts/install-openobserve-daemon.sh' },
        { status: 409 },
      )
    }
    // enable first: bootstrap fails with EIO on a disabled service.
    await launchctl('enable', target)
    await launchctl('bootstrap', domain, plist) // "already bootstrapped" is fine
    await launchctl('kickstart', target)        // start now if not running
    const up = (await launchctl('print', target)).ok
    if (!up) {
      return Response.json({ error: 'OpenObserve failed to start — check ~/.meridian/logs/openobserve-error.log' }, { status: 500 })
    }
    return Response.json({ ok: true, running: true })
  }

  // Stop, and disable so RunAtLoad does not restart it at next login.
  await launchctl('bootout', target) // "not loaded" is fine
  await launchctl('disable', target)
  return Response.json({ ok: true, running: false })
}
