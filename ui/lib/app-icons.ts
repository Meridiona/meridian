//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Resolves an app's real macOS icon (extracted from its own .app bundle by
// the `get_app_icon` Tauri command — see tray/src-tauri/src/commands/app_icons.rs)
// for `AppGlyph` (components/atoms.tsx). One in-flight/resolved lookup per
// distinct app name, shared module-wide so every AppGlyph instance for the
// same app (the "Time by app" chart, the Hour detail panel, Tasks pane, …)
// piggybacks on a single invoke() round trip instead of one each.

import { useEffect, useState } from 'react'
import { invoke, assetUrl, isTauri } from '@/lib/bridge'
import { LruMap } from '@/lib/lru'

type CacheEntry = string | null | Promise<string | null>

// LRU-capped: this webview session never reloads, so an uncapped Map would
// grow by one entry per distinct app name ever seen for the app's whole
// lifetime. 100 is far beyond realistic usage — a hygiene bound, not a fix
// for an observed runaway.
const cache = new LruMap<string, CacheEntry>(100)

function resolve(appName: string): CacheEntry {
  const cached = cache.get(appName)
  if (cached !== undefined) return cached

  const pending = invoke<string | null>('get_app_icon', { appName })
    .then((path) => {
      const url = path ? assetUrl(path) : null
      cache.set(appName, url)
      return url
    })
    .catch(() => {
      cache.set(appName, null)
      return null
    })
  cache.set(appName, pending)
  return pending
}

/** The real app icon URL for `appName`, or `null` while unresolved/unavailable
 *  (uninstalled app, non-Tauri context, extraction failure) — callers render
 *  their existing fallback (brand wordmark / letter monogram) until/unless
 *  this resolves. */
export function useAppIconUrl(appName: string | null | undefined): string | null {
  const [url, setUrl] = useState<string | null>(null)

  useEffect(() => {
    if (!appName || !isTauri()) {
      setUrl(null)
      return
    }
    let cancelled = false
    const entry = resolve(appName)
    if (typeof entry === 'string' || entry === null) {
      setUrl(entry)
    } else {
      setUrl(null)
      entry.then((resolved) => { if (!cancelled) setUrl(resolved) })
    }
    return () => { cancelled = true }
  }, [appName])

  return url
}
