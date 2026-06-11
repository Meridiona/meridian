//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { NextResponse } from 'next/server'
import { spawn } from 'child_process'
import fs from 'fs'
import os from 'os'
import path from 'path'
import { logger, withSpan } from '@/lib/observability'

export const dynamic = 'force-dynamic'

// The command a user runs to update. `meridian update` self-elevates only the
// npm step, preserves creds + venv, and restarts the daemons.
const UPDATE_CMD = 'meridian update'

/**
 * Launch `meridian update` in a visible Terminal window. We do NOT run the
 * update from this server process: the update restarts the daemons (and may
 * prompt for a password on a root-owned npm prefix), so it must run in an
 * interactive terminal the user can see — not silently inside the dashboard.
 *
 * Mechanism: write a tiny .command script and `open -a Terminal` it. This uses
 * LaunchServices (no AppleEvents/Automation permission prompt, unlike osascript)
 * and runs in an interactive login shell so `meridian` is on PATH. The response
 * also returns the raw command so the UI can show a copyable fallback if the
 * user's environment blocks launching Terminal.
 */
export async function POST(request: Request) {
  const url = new URL(request.url)
  return withSpan('api.update', { route: url.pathname }, async () => {
    const script = [
      '#!/bin/bash',
      'echo "→ Updating Meridian…"',
      'echo',
      UPDATE_CMD,
      'status=$?',
      'echo',
      'if [ $status -eq 0 ]; then echo "✓ Update complete — you can close this window."; else echo "✗ Update failed (exit $status). See output above."; fi',
      '',
    ].join('\n')

    const scriptPath = path.join(os.tmpdir(), 'meridian-update.command')
    try {
      fs.writeFileSync(scriptPath, script, { mode: 0o755 })
      const child = spawn('open', ['-a', 'Terminal', scriptPath], {
        detached: true,
        stdio: 'ignore',
      })
      child.unref()
      logger.info({ scriptPath }, 'launched meridian update in Terminal')
      return NextResponse.json({ launched: true, command: UPDATE_CMD })
    } catch (err) {
      // Couldn't launch Terminal — the UI falls back to showing the command.
      logger.warn({ err: String(err) }, 'could not launch Terminal for update')
      return NextResponse.json({ launched: false, command: UPDATE_CMD }, { status: 200 })
    }
  })
}
