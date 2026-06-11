//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { NextResponse } from 'next/server'
import fs from 'fs'
import os from 'os'
import path from 'path'
import { logger, withSpan } from '@/lib/observability'

export const dynamic = 'force-dynamic'

const NPM_PKG = '@meridiona/meridian'
// Package-root endpoint with the abbreviated-metadata accept header (below):
// returns dist-tags without the full per-version payload — light and correct.
// (The /<pkg>/latest endpoint 406s with the abbreviated header.)
const NPM_PKG_URL = `https://registry.npmjs.org/${NPM_PKG}`
const CHECK_TTL_MS = 60 * 60 * 1000 // re-check npm at most hourly

// In-memory cache of the npm result — persists across requests in the same
// server process, so a dashboard left open doesn't hammer the registry.
let cache: { latest: string | null; checkedAt: number } | null = null

interface VersionInfo {
  current: string
  latest: string | null
  updateAvailable: boolean
  checkedAt: string | null
}

/** Installed bundle version from ~/.meridian/app/VERSION; 'dev' when absent. */
function readCurrentVersion(): string {
  try {
    const v = fs.readFileSync(path.join(os.homedir(), '.meridian', 'app', 'VERSION'), 'utf8').trim()
    if (v) return v
  } catch {
    /* not installed via bundle (e.g. dev) — fall through */
  }
  return process.env.MERIDIAN_VERSION?.trim() || 'dev'
}

/** Compare dotted numeric versions. Returns true if `latest` > `current`. */
function isNewer(latest: string, current: string): boolean {
  if (current === 'dev') return false
  const norm = (v: string) => v.replace(/^v/, '').split('-')[0].split('.').map((n) => parseInt(n, 10) || 0)
  const a = norm(latest)
  const b = norm(current)
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const d = (a[i] ?? 0) - (b[i] ?? 0)
    if (d !== 0) return d > 0
  }
  return false
}

/** Fetch the latest published version from npm, cached for CHECK_TTL_MS. */
async function fetchLatest(): Promise<{ latest: string | null; checkedAt: number }> {
  if (cache && Date.now() - cache.checkedAt < CHECK_TTL_MS) return cache
  try {
    const res = await fetch(NPM_PKG_URL, {
      headers: { accept: 'application/vnd.npm.install-v1+json' },
      signal: AbortSignal.timeout(5000),
    })
    if (!res.ok) throw new Error(`npm registry ${res.status}`)
    const body = (await res.json()) as { 'dist-tags'?: { latest?: string } }
    cache = { latest: body['dist-tags']?.latest ?? null, checkedAt: Date.now() }
  } catch (err) {
    // Network/offline/registry error — keep any prior result, else null. Never
    // throw: an update check must not break the dashboard.
    logger.warn({ err: String(err) }, 'version check: npm registry unreachable')
    cache = cache ?? { latest: null, checkedAt: Date.now() }
  }
  return cache
}

export async function GET(request: Request) {
  const url = new URL(request.url)
  return withSpan('api.version', { route: url.pathname }, async () => {
    const current = readCurrentVersion()
    const { latest, checkedAt } = await fetchLatest()
    const info: VersionInfo = {
      current,
      latest,
      updateAvailable: latest != null && isNewer(latest, current),
      checkedAt: checkedAt ? new Date(checkedAt).toISOString() : null,
    }
    return NextResponse.json(info)
  })
}
