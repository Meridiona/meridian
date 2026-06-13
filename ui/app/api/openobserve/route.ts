//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// OpenObserve service control for the "OpenObserve Export" toggle.
//
//   POST { enabled: true }
//     - agent already installed → start it (cred-sync on first boot, then
//       enable/bootstrap/kickstart) and confirm it is actually serving
//     - agent NOT installed (fresh machine) → kick off install-openobserve-
//       daemon.sh in the background (downloads the binary, writes the plist,
//       reads creds from settings.json, starts the service) and return
//       { installing: true }; the client polls GET until it is reachable
//   POST { enabled: false } → stop + disable (persists across logins)
//   GET → { installed, running, reachable, installing, failed, error }
//
// The toggle gates the SERVICE, not just the daemon's exporters: with export
// off there is no idle OpenObserve server holding :5080 and its memory caps.

import { execFile, spawn } from 'child_process'
import { promisify } from 'util'
import fs from 'fs'
import os from 'os'
import path from 'path'
import { readSettings } from '@/lib/settings'

export const dynamic = 'force-dynamic'

const execFileP = promisify(execFile)
const LABEL = 'com.meridiona.openobserve'
const HEALTHZ = 'http://localhost:5080/healthz'
// Status file written by the background installer wrapper: "installing" while
// it runs, then "exit:<code>" when done. Lives outside any app dir so it
// survives bundle re-installs.
const STATUS_FILE = path.join(os.homedir(), '.meridian', '.oo-install.status')
const INSTALL_LOG = path.join(os.homedir(), '.meridian', 'logs', 'openobserve-install.log')

function plistPath(): string {
  return path.join(/*turbopackIgnore: true*/ os.homedir(), 'Library', 'LaunchAgents', `${LABEL}.plist`)
}

async function launchctl(...args: string[]): Promise<{ ok: boolean; err?: string }> {
  try {
    await execFileP('launchctl', args)
    return { ok: true }
  } catch (e) {
    return { ok: false, err: e instanceof Error ? e.message : String(e) }
  }
}

async function reachable(): Promise<boolean> {
  try {
    const r = await fetch(HEALTHZ, { signal: AbortSignal.timeout(1000) })
    return r.ok
  } catch {
    return false
  }
}

// The configured OTLP traces URL (settings override → local default). Used as
// the authenticated probe target below.
function tracesUrl(): string {
  const { otlp_endpoint } = readSettings()
  return otlp_endpoint && otlp_endpoint.trim()
    ? otlp_endpoint
    : 'http://localhost:5080/api/default/v1/traces'
}

// Do the given credentials actually authenticate against the running
// OpenObserve? Good auth → 200, wrong auth → 401/403. Lets us catch the case
// where OO was already initialised with a different root account (changing the
// password in Settings after first boot is a no-op — OO only reads
// ZO_ROOT_USER_* on first boot — so the new creds would silently not work).
async function authOk(email: string, password: string): Promise<boolean> {
  try {
    const auth = Buffer.from(`${email}:${password}`).toString('base64')
    const r = await fetch(tracesUrl(), {
      method: 'POST',
      headers: { Authorization: `Basic ${auth}`, 'Content-Type': 'application/x-protobuf' },
      body: new Uint8Array(),
      signal: AbortSignal.timeout(2000),
    })
    return r.status !== 401 && r.status !== 403
  } catch {
    return false
  }
}

// Resolve install-openobserve-daemon.sh across install types: bundle first
// (~/.meridian/app), then walk up from cwd for a source checkout.
function resolveInstaller(): string | null {
  const candidates = [
    path.join(/*turbopackIgnore: true*/ os.homedir(), '.meridian', 'app', 'scripts', 'install-openobserve-daemon.sh'),
  ]
  let dir = process.cwd()
  for (let i = 0; i < 6; i++) {
    candidates.push(path.join(/*turbopackIgnore: true*/ dir, 'scripts', 'install-openobserve-daemon.sh'))
    const parent = path.dirname(/*turbopackIgnore: true*/ dir)
    if (parent === dir) break
    dir = parent
  }
  return candidates.find(p => fs.existsSync(/*turbopackIgnore: true*/ p)) ?? null
}

// POSIX single-quote escape for safe interpolation into a bash -c string.
function sq(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`
}

function startBackgroundInstall(script: string): { ok: boolean; error?: string } {
  try {
    fs.mkdirSync(/*turbopackIgnore: true*/ path.dirname(INSTALL_LOG), { recursive: true })
    fs.writeFileSync(/*turbopackIgnore: true*/ STATUS_FILE, 'installing')
    // Wrapper runs the installer, tees output to the log, and records the exit
    // code in the status file. Detached + unref so it outlives this request.
    const cmd = `${sq(script)} >> ${sq(INSTALL_LOG)} 2>&1; printf 'exit:%s' "$?" > ${sq(STATUS_FILE)}`
    const child = spawn('bash', ['-c', cmd], { detached: true, stdio: 'ignore' })
    child.unref()
    return { ok: true }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) }
  }
}

function readStatus(): string | null {
  try {
    return fs.readFileSync(/*turbopackIgnore: true*/ STATUS_FILE, 'utf-8').trim()
  } catch {
    return null
  }
}

export async function GET() {
  const installed = fs.existsSync(/*turbopackIgnore: true*/ plistPath())
  const status = readStatus()
  const installing = status === 'installing'
  const up = await reachable()
  let failed = false
  let error: string | undefined
  if (status && status.startsWith('exit:') && status !== 'exit:0' && !up) {
    failed = true
    error = `OpenObserve install failed (code ${status.slice('exit:'.length)}) — see ${INSTALL_LOG}`
  }
  return Response.json({ installed, installing, reachable: up, failed, error })
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
  const plist = plistPath()

  if (body.enabled) {
    // Fresh machine: no launchd agent yet. Bootstrap from zero — the installer
    // downloads the binary, writes the plist, reads creds from settings.json
    // (the UI saved them just before calling this), and starts the service.
    // It is slow (binary download), so run it in the background and let the
    // client poll GET for readiness.
    if (!fs.existsSync(/*turbopackIgnore: true*/ plist)) {
      const script = resolveInstaller()
      if (!script) {
        return Response.json(
          { error: 'OpenObserve installer not found (scripts/install-openobserve-daemon.sh)' },
          { status: 500 },
        )
      }
      const started = startBackgroundInstall(script)
      if (!started.ok) {
        return Response.json({ error: started.error ?? 'failed to start installer' }, { status: 500 })
      }
      return Response.json({ ok: true, installing: true })
    }

    // Sync the UI-entered credentials into the plist's ZO_ROOT_USER_* env vars
    // BEFORE the service's first start. OpenObserve creates its root account
    // from these on its FIRST boot only — once the data dir is populated they
    // are ignored, so we skip the patch (and the restart it requires) to avoid
    // bouncing an already-running instance.
    const dataDir = path.join(/*turbopackIgnore: true*/ os.homedir(), '.openobserve', 'data')
    let initialised = false
    try {
      initialised = fs.readdirSync(/*turbopackIgnore: true*/ dataDir).length > 0
    } catch { /* no data dir yet — first boot */ }
    const { oo_email, oo_password } = readSettings()
    if (!initialised && oo_email && oo_password) {
      await launchctl('bootout', target) // ensure next bootstrap reads the fresh plist
      // launchd tears the entry down asynchronously; bootstrap fails with EIO
      // while it lingers. Poll until gone (max ~5 s).
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
    // Confirm OO is actually SERVING, not merely loaded — launchctl reports a
    // job as present the moment it is bootstrapped, well before it binds :5080.
    let up = false
    for (let i = 0; i < 20; i++) {
      if (await reachable()) { up = true; break }
      await new Promise(r => setTimeout(r, 500))
    }
    if (!up) {
      return Response.json(
        { error: 'OpenObserve did not become reachable — see ~/.meridian/logs/openobserve-error.log' },
        { status: 500 },
      )
    }
    // Already-initialised instance: verify the credentials in Settings actually
    // log in. If they don't, the user almost certainly changed email/password
    // after OO's first boot — which OO ignores — so fail loudly instead of
    // reporting success while export silently 401s.
    if (initialised && oo_email && oo_password && !(await authOk(oo_email, oo_password))) {
      return Response.json(
        {
          error:
            'OpenObserve is already initialised with a different login. Enter the existing ' +
            'OpenObserve credentials, or reset ~/.openobserve/data to start over with new ones.',
        },
        { status: 409 },
      )
    }
    return Response.json({ ok: true, running: true })
  }

  // Stop, and disable so RunAtLoad does not restart it at next login.
  await launchctl('bootout', target) // "not loaded" is fine
  await launchctl('disable', target)
  return Response.json({ ok: true, running: false })
}
