//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// Guards the calling convention of `bridge.mutate`.
//
// `mutate(apiPath, command, body)` does NOT forward `body` as the command's
// arguments - it wraps them: `invoke(command, { body })` (bridge.ts). So a
// command reached through `mutate` MUST declare a single `body:` parameter.
// Most write commands do; a few older ones take their args at the top level
// (`delete_notice(notice_id: String)`) and must be reached with `invoke`
// directly instead.
//
// Getting this wrong produces the worst possible failure: Tauri fails argument
// deserialization, the promise rejects, and a `.catch()` on the call site
// swallows it. The control renders, is clickable, and does nothing - no error,
// no log, no failing test. That is how the NoticeBar dismiss button shipped
// dead in its first draft, and it is the same shape as the `window.confirm`
// Repair Database button that shipped dead in 1.83.0 (see CLAUDE.md).
//
// There is no type checking across the JS -> Rust hop, so this scan is the only
// thing standing between a mismatched call and a silently inert control.
import { describe, it, expect } from 'bun:test'
import { readFileSync, readdirSync, statSync } from 'fs'
import { resolve } from 'path'

// `resolve`, not string concatenation. An unnormalised `dir + '/../..'` keeps
// the `/__tests__/` segment in every descendant path, so the "skip test files"
// filter below silently discarded ALL 204 sources and the main assertion
// passed over an empty list. The `sites.length > 10` floor is what caught it.
const repoRoot = resolve(import.meta.dir, '../..')

function walk(dir: string, ext: string[], out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name === '.next' || name === 'out') continue
    const p = dir + '/' + name
    if (statSync(p).isDirectory()) walk(p, ext, out)
    else if (ext.some(e => name.endsWith(e))) out.push(p)
  }
  return out
}

/** Every `mutate(…, 'command', …)` call site in the dashboard. */
function mutateCallSites(): { file: string; command: string }[] {
  const out: { file: string; command: string }[] = []
  for (const f of walk(repoRoot + '/ui', ['.ts', '.tsx'])) {
    if (f.includes('/__tests__/')) continue
    const src = readFileSync(f, 'utf8')
    for (const m of src.matchAll(/\bmutate(?:<[^>]*>)?\(\s*'[^']*'\s*,\s*'([^']+)'/g)) {
      out.push({ file: f.slice(repoRoot.length + 1), command: m[1] })
    }
  }
  return out
}

// Parameter types Tauri injects itself rather than reading from the JS call.
// They are invisible to the caller, so they do not affect whether `mutate`
// works. `State<…>` covers the DB pool; AppHandle/Window/WebviewWindow cover
// the handle-taking commands.
const INJECTED = /^(?:_?\w+\s*:\s*)?(?:tauri::)?(?:State\s*<|AppHandle|Window|WebviewWindow)/

/**
 * Commands the tray exposes → the names of the arguments they expect *from JS*.
 *
 * The distinction matters: `mutate` sends exactly one key, `body`. A command
 * with no JS-facing args at all (`sync_tasks()`) is fine, because Tauri ignores
 * the surplus key. A command with a JS-facing arg that is not `body`
 * (`delete_notice(notice_id)`) fails deserialization.
 */
function commandJsArgs(): Map<string, string[]> {
  const map = new Map<string, string[]>()
  for (const f of walk(repoRoot + '/tray/src-tauri/src', ['.rs'])) {
    const src = readFileSync(f, 'utf8')
    // `pub async fn name(` … up to the closing paren of the signature.
    for (const m of src.matchAll(/pub (?:async )?fn ([a-z0-9_]+)\s*\(([\s\S]*?)\)\s*->/g)) {
      const [, name, params] = m
      if (map.has(name)) continue
      const jsArgs = splitParams(params)
        .filter(p => p.trim() && !INJECTED.test(p.trim()))
        .map(p => p.split(':')[0].trim())
      map.set(name, jsArgs)
    }
  }
  return map
}

/** Split a Rust parameter list on top-level commas (generics contain commas). */
function splitParams(params: string): string[] {
  const out: string[] = []
  let depth = 0
  let cur = ''
  for (const ch of params) {
    if (ch === '<' || ch === '(' || ch === '[') depth++
    else if (ch === '>' || ch === ')' || ch === ']') depth--
    if (ch === ',' && depth === 0) { out.push(cur); cur = '' } else cur += ch
  }
  if (cur.trim()) out.push(cur)
  return out
}

describe('bridge.mutate body contract', () => {
  const sites = mutateCallSites()
  const jsArgs = commandJsArgs()

  it('finds the call sites and the command signatures at all', () => {
    // Both parsers failing would make the assertion below pass over nothing —
    // and one of them did exactly that on the first draft (see repoRoot).
    expect(sites.length).toBeGreaterThan(10)
    expect(jsArgs.size).toBeGreaterThan(50)
    expect(jsArgs.get('update_settings')).toEqual(['body'])
    // The two shapes that are legitimately reachable via mutate despite having
    // no `body`: no JS args at all, and only an injected AppHandle.
    expect(jsArgs.get('sync_tasks')).toEqual([])
    expect(jsArgs.get('open_setup')).toEqual([])
    // …and the shape that is not.
    expect(jsArgs.get('delete_notice')).toEqual(['notice_id'])
  })

  it('no mutate() call reaches a command expecting a non-body argument', () => {
    const offenders = sites
      .filter(s => {
        const args = jsArgs.get(s.command)
        // Unknown command → not our business (may live outside the scanned tree).
        if (!args) return false
        return args.some(a => a !== 'body')
      })
      .map(s => `${s.file} calls mutate(…, '${s.command}', …), but ${s.command} expects ${jsArgs.get(s.command)!.join(', ')} at the top level - mutate only ever sends { body }, so use invoke() directly`)
    expect(offenders).toEqual([])
  })

  it('the NoticeBar dismiss button reaches delete_notice the working way', () => {
    // Pinned specifically: this is the site that shipped dead, and the generic
    // scan above would stop covering it the moment someone renames the param.
    const src = readFileSync(repoRoot + '/ui/components/NoticeBar.tsx', 'utf8')
    expect(src).toContain("invoke('delete_notice'")
    expect(src).not.toContain("mutate('/api/notices'")
  })

  it('the dismiss failure path is not silent', () => {
    // `catch {}` on a write is what made the dead button invisible locally.
    const src = readFileSync(repoRoot + '/ui/components/NoticeBar.tsx', 'utf8')
    const btn = src.slice(src.indexOf('function DismissButton'))
    expect(btn).toContain('console.error')
  })
})
