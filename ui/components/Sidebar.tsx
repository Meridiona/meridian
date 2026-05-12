// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import { fmtDurDecimal, AppGlyph, TaskKey, LiveDot, useTick } from '@/components/atoms'

type View = 'today' | 'tasks' | 'queue' | 'sessions' | 'week'

interface Props {
  view: View
  onNavigate: (v: View) => void
  onOpenCmd: () => void
  queueCount: number
}

interface ActiveInfo {
  app_name: string
  started_at: string
  elapsed_s: number
  category: string
}

function ActiveSessionPill({ info, onClick }: { info: ActiveInfo; onClick: () => void }) {
  const tick = useTick(1)
  const elapsed = info.elapsed_s + tick

  return (
    <button onClick={onClick} className="m-3 p-3 rounded-lg text-left transition-colors"
      style={{ background: 'var(--surface)', border: '1px solid var(--rule)' }}>
      <div className="flex items-center gap-2">
        <LiveDot size={8} />
        <span className="text-[10px] uppercase tracking-[0.16em]" style={{ color: 'var(--ink-3)' }}>Now</span>
        <span className="ml-auto font-mono tnum text-[11px]" style={{ color: 'var(--ink-2)' }}>
          {fmtDurDecimal(elapsed)}
        </span>
      </div>
      <div className="flex items-center gap-2 mt-2">
        <AppGlyph app={info.app_name} size={18} />
        <span className="text-[12px] truncate" style={{ color: 'var(--ink)' }}>{info.app_name}</span>
      </div>
    </button>
  )
}

export default function Sidebar({ view, onNavigate, onOpenCmd, queueCount }: Props) {
  const [active, setActive] = useState<ActiveInfo | null>(null)

  useEffect(() => {
    function load() {
      fetch('/api/active').then(r => r.json()).then((d: ActiveInfo | null) => setActive(d))
    }
    load()
    const id = setInterval(load, 30_000)
    return () => clearInterval(id)
  }, [])

  const items: Array<{ id: View; label: string; kbd: string; badge?: number }> = [
    { id: 'today',    label: 'Today',    kbd: '1' },
    { id: 'tasks',    label: 'Tasks',    kbd: '2' },
    { id: 'queue',    label: 'Queue',    kbd: '3', badge: queueCount || undefined },
    { id: 'sessions', label: 'Sessions', kbd: '4' },
    { id: 'week',     label: 'Week',     kbd: '5' },
  ]

  return (
    <aside className="w-[240px] shrink-0 sticky top-0 self-start h-screen flex flex-col rule-r"
      style={{ borderRightColor: 'var(--rule)', background: 'var(--paper)' }}>
      {/* Wordmark */}
      <div className="px-6 py-7">
        <div className="flex items-center gap-2">
          <span className="inline-block w-2.5 h-2.5 rounded-full live-dot" style={{ background: 'var(--accent)' }} />
          <span className="italic text-[22px] leading-none tracking-tight" style={{ color: 'var(--ink)', fontFamily: "'Instrument Serif', Georgia, serif" }}>meridian</span>
        </div>
        <p className="text-[10px] uppercase tracking-[0.2em] mt-2" style={{ color: 'var(--ink-3)' }}>local · v0.3</p>
      </div>

      {/* Nav */}
      <nav className="flex-1 px-3">
        {items.map(it => {
          const isActive = view === it.id
          return (
            <button key={it.id}
              onClick={() => onNavigate(it.id)}
              className="w-full flex items-center gap-3 px-3 py-2 rounded-md text-left transition-colors mb-px"
              style={{
                background: isActive ? 'var(--surface-2)' : 'transparent',
                color: isActive ? 'var(--ink)' : 'var(--ink-2)',
              }}>
              <span className="text-[13px] flex-1">{it.label}</span>
              {it.badge != null && (
                <span className="text-[10px] font-mono tnum px-1.5 py-0.5 rounded"
                  style={{ background: 'var(--accent)', color: 'var(--paper)' }}>{it.badge}</span>
              )}
              <span className="kbd">{it.kbd}</span>
            </button>
          )
        })}

        <button onClick={onOpenCmd}
          className="w-full flex items-center gap-3 px-3 py-2 rounded-md text-left mt-4"
          style={{ color: 'var(--ink-3)' }}>
          <span className="text-[13px] flex-1">Quick switch</span>
          <span className="kbd">⌘</span>
          <span className="kbd">K</span>
        </button>
      </nav>

      {/* Active session pill */}
      {active && <ActiveSessionPill info={active} onClick={() => onNavigate('today')} />}
    </aside>
  )
}
