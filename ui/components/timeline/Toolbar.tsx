//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The 60px top toolbar (STYLESHEET.md §7): date navigation + capture
// pause/resume pill on the left, the Meridian mark + wordmark centered, and
// the theme swatches / auto-derived Connected-Solo pill / settings gear on
// the right. A 3-column grid (1fr / auto / 1fr) keeps the brand lockup truly
// centered regardless of how wide the left/right content is — same technique
// as the tray popover's header. The connected/solo pill is NOT a manual
// toggle — it reflects real get_integrations state from useTimelineData.

'use client'

import { useCallback, useEffect, useState } from 'react'
import { load, invoke } from '@/lib/bridge'
import { formatDayLabel } from './types'
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
    <div className="grid items-center gap-4 px-6 shrink-0 border-b"
      style={{ height: 60, gridTemplateColumns: '1fr auto 1fr', borderColor: 'var(--t-hair)', background: 'var(--toolbar-bg)' }}>
      <div className="flex items-center gap-4 min-w-0">
        {/* date nav */}
        <div className="flex items-center gap-1">
          <NavBtn glyph="‹" label="Previous day" onClick={() => onShiftDay(-1)} />
          <span className="mt-toolbar-date px-2 min-w-32 text-center whitespace-nowrap" style={{ color: 'var(--t-title)' }}>
            {isToday ? 'Today' : formatDayLabel(day)}
          </span>
          <NavBtn glyph="›" label="Next day" onClick={() => onShiftDay(1)} disabled={isToday} />
        </div>

        {/* capture pill */}
        <button onClick={toggleCapture} disabled={running === null}
          className="inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 bg-ctrl shrink-0"
          style={{ border: '1px solid var(--t-ctrl-border)', opacity: running === null ? 0.6 : 1 }}>
          <span className="inline-block w-2 h-2 rounded-full"
            style={{ background: running ? 'var(--color-state-approved)' : 'var(--color-state-pending)' }} />
          <span className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
            {running === null ? 'Capture' : running ? 'Capturing' : 'Paused'}
          </span>
        </button>
      </div>

      {/* brand — centered regardless of left/right content width. icon.png's
          mark occupies ~60% of its 512x512 canvas (app-icon safe-area
          padding), so it's cropped to fill the 22px box via a scaled-up
          background-image rather than shown at its native padded ratio. */}
      <div className="flex items-center gap-2 justify-self-center">
        <span className="shrink-0 rounded-md" aria-hidden="true" style={{
          width: 22, height: 22,
          backgroundImage: 'url(/icon.png)',
          backgroundSize: '166% 166%',
          backgroundPosition: 'center',
          backgroundRepeat: 'no-repeat',
        }} />
        <span className="mt-title" style={{ color: 'var(--t-title)' }}>Meridian</span>
      </div>

      <div className="ml-auto flex items-center gap-4">
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
