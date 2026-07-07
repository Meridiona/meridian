//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The "Export Diagnostics" action: bundles the local telemetry spool + recent
// JSONL logs into a .tar.gz and reveals it in Finder, via the
// `export_diagnostics_bundle` Tauri command (tray/src-tauri/src/commands/diagnostics.rs).
// Replaces the old useApplyObservability install/start/stop flow — the shipped
// app never installs/runs a local OpenObserve service, so there's nothing to
// apply/reload here, just a one-shot export.

'use client'

import { useCallback, useState } from 'react'
import { load } from '@/lib/bridge'

export type ExportStatus = 'idle' | 'exporting' | 'done' | 'error'

export function useExportDiagnostics() {
  const [status, setStatus] = useState<ExportStatus>('idle')
  const [path, setPath] = useState<string | null>(null)
  const [errorMsg, setErrorMsg] = useState<string | null>(null)

  const exportBundle = useCallback(async () => {
    setStatus('exporting')
    setErrorMsg(null)
    try {
      const bundlePath = await load<string>('/api/diagnostics/export', 'export_diagnostics_bundle')
      setPath(bundlePath)
      setStatus('done')
      setTimeout(() => setStatus('idle'), 4000)
    } catch (e) {
      setErrorMsg(e instanceof Error ? e.message : 'Export failed')
      setStatus('error')
      setTimeout(() => { setStatus('idle'); setErrorMsg(null) }, 6000)
    }
  }, [])

  return { status, path, errorMsg, exportBundle }
}
