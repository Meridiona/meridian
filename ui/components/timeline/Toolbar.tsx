//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The ~56px top toolbar: date navigation, a capture pause/resume pill, the
// theme swatches, an auto-derived Connected/Solo pill, and a settings gear.
// The connected/solo pill is NOT a manual toggle — it reflects real
// get_integrations state from useTimelineData.

'use client'

import { useCallback, useEffect, useState } from 'react'
import { load, invoke } from '@/lib/bridge'
import { ThemeSwatches } from './ThemeSwatches'

export function Toolbar({
  day, isToday, onShiftDay, isSolo, connectedProviderName, onOpenSettings,
}: {
  day: string
  isToday: boolean
  onShiftDay: (delta: number) => void
  isSolo: boolean
  connectedProviderName: string | null
  onOpenSettings: () => void
}) {
  const [running, setRunning] = useState<boolean | null>(null)

  const refreshStatus = useCallback(() => {
    load<{ running: boolean }>('/api/daemon/status', 'get_daemon_status')
      .then(s => setRunning(s.running))
      .catch(() => setRunning(null))
  }, [])

  useEffect(() => {
    refreshStatus()
    const id = setInterval(refreshStatus, 30_000)
    return () => clearInterval(id)
  }, [refreshStatus])

  // toggle_daemon pauses (stop) / resumes (start) capture. Optimistic flip.
  async function toggleCapture() {
    if (running === null) return
    const next = !running
    setRunning(next)
    try {
      await invoke('toggle_daemon', { isRunning: running })
    } catch {
      setRunning(running) // revert on failure
    }
    refreshStatus()
  }

  const connectionLabel = isSolo ? 'Solo' : connectedProviderName ?? 'Connected'

  return (
    <div className="flex items-center gap-3 px-4 shrink-0 border-b"
      style={{ height: 56, borderColor: 'var(--t-hair)', background: 'var(--toolbar-bg)' }}>
      {/* date nav */}
      <div className="flex items-center gap-1">
        <NavBtn glyph="‹" label="Previous day" onClick={() => onShiftDay(-1)} />
        <span className="mt-toolbar-date px-2 min-w-24 text-center" style={{ color: 'var(--t-title)' }}>
          {isToday ? 'Today' : day}
        </span>
        <NavBtn glyph="›" label="Next day" onClick={() => onShiftDay(1)} disabled={isToday} />
      </div>

      {/* capture pill */}
      <button onClick={toggleCapture} disabled={running === null}
        className="inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 bg-ctrl"
        style={{ border: '1px solid var(--t-ctrl-border)', opacity: running === null ? 0.6 : 1 }}>
        <span className="inline-block w-2 h-2 rounded-full"
          style={{ background: running ? 'var(--color-state-approved)' : 'var(--color-state-pending)' }} />
        <span className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
          {running === null ? 'Capture' : running ? 'Capturing' : 'Paused'}
        </span>
      </button>

      <div className="ml-auto flex items-center gap-3">
        <ThemeSwatches />
        <span className="w-px h-5" style={{ background: 'var(--t-hair)' }} />
        <span className="inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 bg-ctrl"
          style={{ border: '1px solid var(--t-ctrl-border)' }}>
          <span className="inline-block w-1.5 h-1.5 rounded-full"
            style={{ background: isSolo ? 'var(--t-faint)' : 'var(--color-state-proposal)' }} />
          <span className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>{connectionLabel}</span>
        </span>
        <button onClick={onOpenSettings} aria-label="Settings"
          className="inline-flex items-center justify-center rounded-full bg-ctrl"
          style={{ width: 32, height: 32, border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)' }}>
          <span className="text-[15px]">⚙</span>
        </button>
      </div>
    </div>
  )
}

function NavBtn({ glyph, label, onClick, disabled }: {
  glyph: string; label: string; onClick: () => void; disabled?: boolean
}) {
  return (
    <button onClick={onClick} disabled={disabled} aria-label={label}
      className="inline-flex items-center justify-center rounded-md bg-ctrl"
      style={{ width: 28, height: 28, border: '1px solid var(--t-ctrl-border)', color: disabled ? 'var(--t-faint-2)' : 'var(--t-muted)' }}>
      <span className="text-[15px] leading-none">{glyph}</span>
    </button>
  )
}
