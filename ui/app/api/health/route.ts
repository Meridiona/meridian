// meridian — normalises screenpipe activity into structured app sessions

import { execSync } from 'child_process'
import { NextRequest, NextResponse } from 'next/server'

interface HealthStatus {
  a11y_helper_trusted: boolean
  database_ready?: boolean
  error?: string
}

export async function GET(request: NextRequest): Promise<NextResponse<HealthStatus>> {
  try {
    // Run meridian doctor and parse for a11y_helper.trusted status
    let doctorOutput = ''
    try {
      doctorOutput = execSync('meridian doctor 2>&1', { encoding: 'utf-8', timeout: 5000 })
    } catch (e) {
      // doctor command might fail if database is not ready — that's ok, we detect it below
      doctorOutput = e instanceof Error && 'stdout' in e ? String(e.stdout) : ''
    }

    // Check if a11y_helper is trusted
    const isTrusted = doctorOutput.includes('a11y_helper.trusted') && doctorOutput.includes('✓  a11y_helper.trusted')

    // Check if database is ready (has app_sessions table with recent migrations)
    const databaseReady = doctorOutput.includes('meridian.db_present') && doctorOutput.includes('✓  meridian.db')

    // If database schema is broken, provide helpful guidance
    if (!databaseReady && doctorOutput.includes('SQLITE_ERROR')) {
      return NextResponse.json(
        {
          a11y_helper_trusted: false,
          database_ready: false,
          error: 'Database schema mismatch — run: bash scripts/migrate-db.sh',
        },
        { status: 200 },
      )
    }

    return NextResponse.json({
      a11y_helper_trusted: isTrusted,
      database_ready: databaseReady !== false,
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
