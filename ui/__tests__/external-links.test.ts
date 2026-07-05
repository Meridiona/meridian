//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync, readdirSync, statSync } from 'fs'
import { isExternalHttp, opensExternally, openExternal } from '../lib/bridge'

// Regression guard for "external links do nothing inside the app".
//
// The dashboard renders inside a Tauri WKWebView, which has NO new-window
// handler: a plain `<a target="_blank">` click or a `window.open(url)` call is
// silently dropped — nothing opens, no error. This bit users on the Trello
// connect flow (the "Get it at trello.com/app-key" link was dead, blocking the
// API-key prompt entirely).
//
// The fix has three parts, each guarded here:
//   1. `openExternal` in lib/bridge.ts routes URLs through the opener plugin
//      (`plugin:opener|open_url` — allowed by `opener:default` in the tray's
//      capabilities/default.json) with a window.open fallback for a plain browser.
//   2. `components/ExternalLinks.tsx` — a document-level click interceptor
//      mounted in the root layout, so EVERY external http(s) anchor works
//      without per-component wiring.
//   3. The former direct `window.open(...)` call sites now use `openExternal`.

const uiRoot = import.meta.dir + '/..'

function readSrc(relPath: string): string {
  return readFileSync(uiRoot + '/' + relPath, 'utf8')
}

// ── isExternalHttp — the interceptor's decision function ─────────────────────

describe('isExternalHttp', () => {
  const origin = 'http://localhost:3939'

  it('accepts absolute http(s) URLs on a different origin', () => {
    expect(isExternalHttp('https://trello.com/app-key', origin)).toBe(true)
    expect(isExternalHttp('http://localhost:5080', origin)).toBe(true) // different port = different origin
    expect(isExternalHttp('HTTPS://TRELLO.COM/x', origin)).toBe(true) // scheme is case-insensitive
  })

  it('rejects same-origin, relative, and non-http URLs', () => {
    expect(isExternalHttp('http://localhost:3939/today', origin)).toBe(false)
    expect(isExternalHttp('/today', origin)).toBe(false)
    expect(isExternalHttp('#anchor', origin)).toBe(false)
    expect(isExternalHttp('mailto:a@b.com', origin)).toBe(false)
    expect(isExternalHttp('x-apple.systempreferences:com.apple.x', origin)).toBe(false)
  })

  it('rejects malformed URLs instead of throwing', () => {
    expect(isExternalHttp('http://', origin)).toBe(false)
  })
})

// ── opensExternally — adds mailto:/tel: on top of external http(s) ───────────

describe('opensExternally', () => {
  const origin = 'http://localhost:3939'

  it('accepts mailto: and tel: (WKWebView drops them like target=_blank)', () => {
    expect(opensExternally('mailto:hey@meridiona.com?subject=Bug', origin)).toBe(true)
    expect(opensExternally('tel:+1555000', origin)).toBe(true)
    expect(opensExternally('MAILTO:hey@meridiona.com', origin)).toBe(true)
  })

  it('delegates http(s) decisions to isExternalHttp', () => {
    expect(opensExternally('https://trello.com/app-key', origin)).toBe(true)
    expect(opensExternally('http://localhost:3939/today', origin)).toBe(false)
    expect(opensExternally('/today', origin)).toBe(false)
    expect(opensExternally('x-apple.systempreferences:com.apple.x', origin)).toBe(false)
  })
})

// ── openExternal routes through the opener plugin ─────────────────────────────

describe('lib/bridge.ts openExternal', () => {
  const src = readSrc('lib/bridge.ts')

  it('invokes the opener plugin command inside Tauri', () => {
    expect(src).toContain("invoke('plugin:opener|open_url'")
  })

  it('falls back to window.open in a plain browser', () => {
    expect(src).toMatch(/openExternal[\s\S]*?window\.open\(url/)
  })

  it('falls back to window.open when the plugin invoke rejects', async () => {
    // A rejection that only logs would reproduce the silent dead-link bug this
    // helper fixes, so the .catch() must open the URL anyway. bridge reads
    // `window` at call time, so a stub installed here is picked up.
    const opened: string[] = []
    const g = globalThis as { window?: unknown }
    g.window = {
      __TAURI__: { core: { invoke: () => Promise.reject(new Error('forbidden url')) } },
      open: (url: string) => { opened.push(url); return null },
    }
    const warn = console.warn
    console.warn = () => {}
    try {
      openExternal('https://example.com/x')
      await new Promise((r) => setTimeout(r, 0))
      expect(opened).toEqual(['https://example.com/x'])
    } finally {
      console.warn = warn
      delete g.window
    }
  })
})

// ── The interceptor exists and is mounted globally ────────────────────────────

describe('ExternalLinks interceptor', () => {
  const src = readSrc('components/ExternalLinks.tsx')

  it('listens for document clicks and routes external anchors via openExternal', () => {
    expect(src).toContain("document.addEventListener('click'")
    expect(src).toContain('opensExternally(href, window.location.origin)')
    expect(src).toContain('e.preventDefault()')
    expect(src).toContain('openExternal(href)')
  })

  it('removes the listener on unmount', () => {
    expect(src).toContain("document.removeEventListener('click'")
  })

  it('is mounted in the root layout (covers dashboard AND setup wizard)', () => {
    const layout = readSrc('app/layout.tsx')
    expect(layout).toContain('<ExternalLinks />')
  })
})

// ── No component bypasses openExternal with a raw window.open ─────────────────

describe('no raw window.open in components', () => {
  function walk(dir: string, acc: string[] = []): string[] {
    for (const name of readdirSync(dir)) {
      const p = dir + '/' + name
      if (statSync(p).isDirectory()) walk(p, acc)
      else if (/\.(ts|tsx)$/.test(name)) acc.push(p)
    }
    return acc
  }

  it('every external open goes through openExternal (WKWebView drops window.open)', () => {
    const offenders = ['/components', '/app']
      .flatMap((d) => walk(uiRoot + d))
      .filter((p) => readFileSync(p, 'utf8').includes('window.open('))
      .map((p) => p.slice(uiRoot.length + 1))
    expect(offenders).toEqual([])
  })
})
