//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The Observability "Apply" flow, extracted verbatim from the old
// SettingsView: save otlp/log fields, start/stop the OpenObserve service to
// match the toggle (installing it first on a fresh machine), then SIGHUP the
// daemon and poll until it's back up. Its own hook because the multi-step
// status machine (saving → installing → reloading → done/error) is only used
// by AdvancedSection, unlike the plain save() every other section uses.

'use client'

import { useCallback, useRef, useState } from 'react'
import type { RuntimeSettings } from '@/lib/settings'
import { load, mutate } from '@/lib/bridge'

export type ReloadStatus = 'idle' | 'saving' | 'installing' | 'reloading' | 'done' | 'error'

// Poll GET /api/openobserve until OpenObserve is reachable or the background
// install fails. Returns true when reachable. Up to ~90 s (binary download).
async function pollOpenObserveReady(): Promise<boolean> {
  for (let i = 0; i < 60; i++) {
    try {
      const s = await load<{ reachable?: boolean; failed?: boolean }>(
        '/api/openobserve',
        'get_openobserve_status',
      )
      if (s.reachable) return true
      if (s.failed) return false
    } catch { /* keep polling */ }
    await new Promise(res => setTimeout(res, 1500))
  }
  return false
}

export function useApplyObservability(
  settings: RuntimeSettings | null,
  setSettings: (s: RuntimeSettings) => void,
) {
  const [reloadStatus, setReloadStatus] = useState<ReloadStatus>('idle')
  const [reloadMsg, setReloadMsg] = useState<string | null>(null)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const apply = useCallback(async () => {
    if (!settings) return
    setReloadMsg(null)
    setReloadStatus('saving')
    try {
      const updated = await mutate<RuntimeSettings>('/api/settings', 'update_settings', {
        log_level: settings.log_level,
        otlp_enabled: settings.otlp_enabled,
        otlp_endpoint: settings.otlp_endpoint,
        oo_email: settings.oo_email,
        oo_password: settings.oo_password,
      }, 'PUT')
      setSettings(updated)
    } catch {
      setReloadStatus('error')
      setTimeout(() => setReloadStatus('idle'), 3000)
      return
    }

    // The toggle gates the OpenObserve SERVICE itself, not just the exporters:
    // enabled → start the launchd agent (installing it first on a fresh
    // machine); disabled → stop it (and keep it off across logins). A failed
    // start is a real error the user must see — otherwise "Apply" reports
    // success while OpenObserve is down.
    try {
      // Dual-path: set_openobserve (Rust) in the app, /api/openobserve POST in a
      // browser. mutate throws the server's error text on failure — surface it.
      const ooBody = await mutate<{ installing?: boolean }>(
        '/api/openobserve', 'set_openobserve', { enabled: settings.otlp_enabled })
      // Fresh machine: the server is downloading + installing OpenObserve in
      // the background. Poll until it is reachable (binary download can take
      // ~30 s) before continuing to the daemon reload.
      if (ooBody.installing) {
        setReloadStatus('installing')
        const ready = await pollOpenObserveReady()
        if (!ready) {
          setReloadStatus('error')
          setTimeout(() => setReloadStatus('idle'), 4000)
          return
        }
      }
    } catch (e) {
      // A failed start is a real error the user must see (8 s) — otherwise "Apply"
      // reports success while OpenObserve is down.
      setReloadMsg(e instanceof Error ? e.message : 'OpenObserve start failed')
      setReloadStatus('error')
      setTimeout(() => { setReloadStatus('idle'); setReloadMsg(null) }, 8000)
      return
    }

    setReloadStatus('reloading')
    try {
      // Dual-path: reload_daemon (Rust) in the app, /api/daemon/reload POST in a
      // browser. Both signal SIGHUP; both report "daemon not running" when down.
      await mutate('/api/daemon/reload', 'reload_daemon', {})
    } catch (e) {
      // Daemon not running (e.g. a dev session with the stack down) — settings
      // are saved and read at the next daemon start, so this is NOT an error.
      if (e instanceof Error && e.message.includes('daemon not running')) {
        setReloadStatus('done')
        setTimeout(() => setReloadStatus('idle'), 3000)
        return
      }
      setReloadStatus('error')
      setTimeout(() => setReloadStatus('idle'), 3000)
      return
    }

    // Poll daemon/status until it responds again (daemon restarted).
    // Give the daemon up to 15 s to come back up.
    let attempts = 0
    pollRef.current = setInterval(async () => {
      attempts++
      try {
        const { running } = await load<{ running: boolean }>(
          '/api/daemon/status',
          'get_daemon_status',
        )
        if (running) {
          clearInterval(pollRef.current!)
          pollRef.current = null
          setReloadStatus('done')
          setTimeout(() => setReloadStatus('idle'), 3000)
        }
      } catch { /* keep polling */ }
      if (attempts >= 30) {
        clearInterval(pollRef.current!)
        pollRef.current = null
        setReloadStatus('error')
        setTimeout(() => setReloadStatus('idle'), 3000)
      }
    }, 500)
  }, [settings, setSettings])

  return { reloadStatus, reloadMsg, apply }
}
