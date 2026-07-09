//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Global external-link interceptor. The dashboard runs inside a Tauri
// WKWebView, which has no new-window handler: a plain `<a target="_blank">`
// click — or a mailto:/tel: anchor — is silently dropped: nothing opens, no
// error. `open_external_url` + per-anchor onClick wiring fixed the known
// "Open ↗" links; this listener catches every OTHER external anchor (and any
// future one) at the document level, so new links work without bespoke
// wiring. Anchors already wired call preventDefault() first, so they're
// skipped here — no double-open. In a plain browser (`isTauri()` false) it
// does nothing and anchors behave natively.
//
// Rendered by app/layout.tsx (root layout), so it covers every window that
// serves the Next app — the dashboard and the setup wizard.

import { useEffect } from 'react'
import { isTauri, opensExternally, openExternal } from '@/lib/bridge'

export default function ExternalLinks() {
  useEffect(() => {
    if (!isTauri()) return
    const onClick = (e: MouseEvent) => {
      if (e.defaultPrevented) return
      const el = e.target instanceof Element ? e.target : null
      const href = el?.closest('a[href]')?.getAttribute('href')
      if (!href || !opensExternally(href, window.location.origin)) return
      e.preventDefault()
      openExternal(href)
    }
    document.addEventListener('click', onClick)
    return () => document.removeEventListener('click', onClick)
  }, [])
  return null
}
