//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { NextResponse } from 'next/server'

export const dynamic = 'force-dynamic'

function basicAuth(pat: string): string {
  return 'Basic ' + Buffer.from(`:${pat}`).toString('base64')
}

async function fetchJson<T>(url: string, pat: string): Promise<{ data: T | null; status: number; error?: string }> {
  try {
    const resp = await fetch(url, {
      headers: { Authorization: basicAuth(pat), Accept: 'application/json' },
    })
    if (!resp.ok) {
      const text = await resp.text().catch(() => '')
      return { data: null, status: resp.status, error: text }
    }
    const data = await resp.json() as T
    return { data, status: resp.status }
  } catch (e) {
    return { data: null, status: 0, error: String(e) }
  }
}

interface ProfileResponse { id: string }
interface AccountsResponse { value: Array<{ accountName: string }> }
interface ProjectsResponse { value: Array<{ name: string }> }

// POST { pat } → { orgs: string[] }
// POST { pat, org } → { projects: string[] }
export async function POST(request: Request) {
  let body: Record<string, string>
  try {
    body = await request.json()
  } catch {
    return NextResponse.json({ error: 'invalid JSON' }, { status: 400 })
  }

  const { pat, org } = body
  if (!pat) return NextResponse.json({ error: 'pat is required' }, { status: 400 })

  if (org) {
    // Step 2: list projects for the chosen org
    const { data, status, error } = await fetchJson<ProjectsResponse>(
      `https://dev.azure.com/${encodeURIComponent(org)}/_apis/projects?api-version=7.1`,
      pat,
    )
    if (!data) {
      const msg = status === 401 || status === 403
        ? 'PAT is invalid or lacks Work Items → Read & write scope'
        : `Azure DevOps returned HTTP ${status}`
      return NextResponse.json({ error: msg, detail: error }, { status: 502 })
    }
    const projects = data.value.map(p => p.name).sort((a, b) => a.localeCompare(b))
    return NextResponse.json({ projects })
  }

  // Step 1: look up the PAT owner's member ID, then list their orgs
  const profile = await fetchJson<ProfileResponse>(
    'https://app.vssps.visualstudio.com/_apis/profile/profiles/me?api-version=6.0',
    pat,
  )
  if (!profile.data) {
    const msg = profile.status === 401 || profile.status === 403
      ? 'PAT is invalid or expired — check it and try again'
      : `Azure DevOps profile API returned HTTP ${profile.status}`
    return NextResponse.json({ error: msg, detail: profile.error }, { status: 502 })
  }

  const memberId = profile.data.id
  const accounts = await fetchJson<AccountsResponse>(
    `https://app.vssps.visualstudio.com/_apis/accounts?memberId=${encodeURIComponent(memberId)}&api-version=6.0`,
    pat,
  )
  if (!accounts.data) {
    return NextResponse.json(
      { error: `Could not list organizations (HTTP ${accounts.status})`, detail: accounts.error },
      { status: 502 },
    )
  }

  const orgs = accounts.data.value.map(a => a.accountName).sort((a, b) => a.localeCompare(b))
  return NextResponse.json({ orgs })
}
