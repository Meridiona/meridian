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
import { readSettings } from '@/lib/settings'

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
    // Sync the UI-entered credentials into the plist's ZO_ROOT_USER_* env vars
    // BEFORE the service's first start. OpenObserve creates its root account
    // from these on its FIRST boot only (no user store yet) — this is what
    // lets a first-time user simply pick an email/password in Settings and
    // have it become their OpenObserve login. Once the instance is
    // initialised the env vars are ignored, so we skip the patch AND the
    // bootout/bootstrap cycle it requires — if OO is already running,
    // enabling must not bounce it (the restart window reads as "Apply didn't
    // work" to anyone who clicks "Open OpenObserve" right away).
    const dataDir = path.join(/*turbopackIgnore: true*/ os.homedir(), '.openobserve', 'data')
    let initialised = false
    try {
      initialised = fs.readdirSync(/*turbopackIgnore: true*/ dataDir).length > 0
    } catch { /* no data dir yet — first boot */ }
    const { oo_email, oo_password } = readSettings()
    if (!initialised && oo_email && oo_password) {
      await launchctl('bootout', target) // ensure next bootstrap reads the fresh plist
      // launchd tears the entry down asynchronously; bootstrap fails with EIO
      // while it lingers. Poll until gone (max ~5 s) — same guard as the
      // install script's bootout wait loop.
      for (let i = 0; i < 10; i++) {
        if (!(await launchctl('print', target)).ok) break
        await new Promise(r => setTimeout(r, 500))
      }
      const patch = async (key: string, value: string) => {
        try {
          await execFileP('plutil', ['-replace', `EnvironmentVariables.${key}`, '-string', value, plist])
        } catch { /* plist without EnvironmentVariables dict — leave as installed */ }
      }
      await patch('ZO_ROOT_USER_EMAIL', oo_email)
      await patch('ZO_ROOT_USER_PASSWORD', oo_password)
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
