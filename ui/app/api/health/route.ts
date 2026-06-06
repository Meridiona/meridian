// meridian — normalises screenpipe activity into structured app sessions

import { execSync } from 'child_process'
import { NextRequest, NextResponse } from 'next/server'

interface HealthStatus {
  a11y_helper_trusted?: boolean
  database_ready?: boolean
  error?: string
}

export async function GET(request: NextRequest): Promise<NextResponse<HealthStatus>> {
  try {
    // The UI daemon runs under launchd whose default PATH (/usr/bin:/bin:…) does not
    // include ~/.local/bin where meridian lives. Without augmenting PATH, execSync
    // gets "sh: meridian: command not found" — a non-empty string that falsely
    // triggers the DB-error branch below.
    const home = process.env.HOME ?? ''
    const augmentedPath = [
      `${home}/.local/bin`,
      `${home}/.npm-global/bin`,
      '/usr/local/bin',
      process.env.PATH ?? '',
    ]
      .filter(Boolean)
      .join(':')

    let doctorOutput = ''
    try {
      doctorOutput = execSync('meridian doctor 2>&1', {
        encoding: 'utf-8',
        timeout: 15000,
        env: { ...process.env, PATH: augmentedPath },
      })
    } catch (e) {
      // doctor exits 1 on critical issues — that's a real run; capture stdout.
      doctorOutput = e instanceof Error && 'stdout' in e ? String(e.stdout) : ''
    }

    // Guard: only trust the output if doctor actually ran (vs "command not found").
    const doctorRan = doctorOutput.includes('meridian DB') || doctorOutput.includes('Meridian doctor')

    // Check if a11y_helper is trusted
    const isTrusted = doctorOutput.includes('a11y_helper.trusted') && doctorOutput.includes('✓  a11y_helper.trusted')

    // Check if database is ready. The doctor check is named "meridian DB" (daemon.rs).
    // Rendered (no color, non-TTY): "    ✓  meridian DB              readable"
    const databaseReady = doctorOutput.includes('✓  meridian DB')

    if (doctorRan && !databaseReady) {
      // Distinguish "db not created yet" (fresh install) from "schema too old" (upgrade).
      const dbMissing =
        doctorOutput.includes('not yet created') ||
        doctorOutput.includes('not readable')
      return NextResponse.json(
        {
          a11y_helper_trusted: false,
          database_ready: false,
          error: dbMissing
            ? 'Database not found — start the daemon: launchctl load ~/Library/LaunchAgents/com.meridiona.daemon.plist'
            : 'Database schema mismatch — run: meridian migrate-db',
        },
        { status: 200 },
      )
    }

    // Only report status when doctor actually ran. When it didn't run (binary not in
    // PATH) or timed out with partial output, omit the fields so the UI treats the
    // state as unknown and shows no banner.
    return NextResponse.json({
      a11y_helper_trusted: doctorRan ? isTrusted : undefined,
      database_ready: doctorRan ? databaseReady : undefined,
    })
  } catch (error) {
    // Unexpected error — default to safe state
    const errorMsg = error instanceof Error ? error.message : 'Unknown error'
    return NextResponse.json(
      {
        a11y_helper_trusted: false,
        database_ready: false,
        error: errorMsg,
      },
      { status: 200 }, // Return 200 even on error so UI can handle it gracefully
    )
  }
}
