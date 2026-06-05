// meridian — normalises screenpipe activity into structured app sessions

import { execSync } from 'child_process'
import { NextRequest, NextResponse } from 'next/server'

interface HealthStatus {
  a11y_helper_trusted: boolean
  error?: string
}

export async function GET(request: NextRequest): Promise<NextResponse<HealthStatus>> {
  try {
    // Run meridian doctor and parse for a11y_helper.trusted status
    const doctorOutput = execSync('meridian doctor 2>&1', { encoding: 'utf-8' })

    // Check if a11y_helper is trusted (look for the ✓ checkmark or critical status)
    const isTrusted = doctorOutput.includes('a11y_helper.trusted') && doctorOutput.includes('✓  a11y_helper.trusted')

    return NextResponse.json({
      a11y_helper_trusted: isTrusted,
    })
  } catch (error) {
    // If meridian doctor fails, default to untrusted (safer assumption)
    const errorMsg = error instanceof Error ? error.message : 'Unknown error'
    return NextResponse.json(
      {
        a11y_helper_trusted: false,
        error: errorMsg,
      },
      { status: 200 }, // Return 200 even on error so UI can handle it gracefully
    )
  }
}
