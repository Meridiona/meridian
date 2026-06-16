//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState } from 'react'
import { usePathname, useRouter } from 'next/navigation'
import { fmtDurDecimal, AppGlyph, TaskKey, LiveDot, useTick } from '@/components/atoms'
import { load as loadData } from '@/lib/bridge'

interface Props {
  onOpenCmd: () => void
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

interface VersionInfo {
  current: string
  latest: string | null
  updateAvailable: boolean
}

export default function Sidebar({ onOpenCmd }: Props) {
  const pathname = usePathname()
  const router = useRouter()
  const [active, setActive] = useState<ActiveInfo | null>(null)
  const [ver, setVer] = useState<VersionInfo | null>(null)
  const [updating, setUpdating] = useState(false)

  useEffect(() => {
    function load() {
      // get_active (Rust) in the Tauri window, /api/active in a browser — same shape.
      loadData<ActiveInfo | null>('/api/active', 'get_active').then(setActive).catch(() => {})
    }
    load()
    const id = setInterval(load, 30_000)
    return () => clearInterval(id)
  }, [])

  useEffect(() => {
    fetch('/api/version').then(r => r.json()).then((d: VersionInfo) => setVer(d)).catch(() => {})
  }, [])

  async function runUpdate() {
    setUpdating(true)
    try {
      await fetch('/api/update', { method: 'POST' })
    } catch {
      /* ignore — banner keeps the copyable command as fallback */
    }
  }

  const items: Array<{ route: string; label: string; kbd: string }> = [
    { route: '/today',    label: 'Today',    kbd: '1' },
    { route: '/tasks',    label: 'Tasks',    kbd: '2' },
    { route: '/worklogs', label: 'Worklogs', kbd: '3' },
    { route: '/sessions', label: 'Sessions', kbd: '4' },
    { route: '/week',     label: 'Week',     kbd: '5' },
    { route: '/cleanup',  label: 'Clean-up', kbd: '7' },
  ]

  return (
    <aside className="w-[240px] shrink-0 sticky top-0 self-start h-screen flex flex-col rule-r"
      style={{ borderRightColor: 'var(--rule)', background: 'var(--paper)' }}>
      {/* Wordmark */}
      <div className="px-6 py-7">
        <div className="flex items-center gap-2">
          <span className="inline-block w-2.5 h-2.5 rounded-full live-dot" style={{ background: 'var(--accent)' }} />
          <span className="type-wordmark" style={{ color: 'var(--ink)' }}>meridian</span>
        </div>
        <p className="text-[10px] uppercase tracking-[0.2em] mt-2" style={{ color: 'var(--ink-3)' }}>
          local · v{ver?.current ?? '…'}
        </p>
      </div>

      {/* Update-available banner — notify + one-click (opens Terminal running `meridian update`) */}
      {ver?.updateAvailable && ver.latest && (
        <div className="mx-3 mb-1 p-3 rounded-lg"
          style={{ background: 'var(--surface)', border: '1px solid var(--accent)' }}>
          <div className="flex items-center gap-2">
            <span className="text-[12px]" style={{ color: 'var(--accent)' }}>↑</span>
            <span className="text-[11px]" style={{ color: 'var(--ink)' }}>
              Update available
            </span>
            <span className="ml-auto font-mono tnum text-[10px]" style={{ color: 'var(--ink-3)' }}>
              v{ver.current} → v{ver.latest}
            </span>
          </div>
          <button onClick={runUpdate} disabled={updating}
            className="w-full mt-2 px-2 py-1.5 rounded-md text-[11px] transition-colors"
            style={{ background: 'var(--accent)', color: 'var(--paper)', opacity: updating ? 0.6 : 1 }}>
            {updating ? 'Opening Terminal…' : 'Update now'}
          </button>
          {updating && (
            <p className="text-[10px] mt-1.5 font-mono" style={{ color: 'var(--ink-3)' }}>
              or run <span style={{ color: 'var(--ink-2)' }}>meridian update</span>
            </p>
          )}
        </div>
      )}

      {/* Nav */}
      <nav className="flex-1 px-3">
        {items.map(it => {
          const isActive = pathname === it.route
          return (
            <button key={it.route}
              onClick={() => router.push(it.route)}
              className="w-full flex items-center gap-3 px-3 py-2 rounded-md text-left transition-colors mb-px"
              style={{
                background: isActive ? 'var(--surface-2)' : 'transparent',
                color: isActive ? 'var(--ink)' : 'var(--ink-2)',
              }}>
              <span className="text-[13px] flex-1">{it.label}</span>
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

      {/* Settings link */}
      <div className="px-3 pb-4">
        <button onClick={() => router.push('/settings')}
          className="w-full flex items-center gap-3 px-3 py-2 rounded-md text-left transition-colors"
          style={{
            background: pathname === '/settings' ? 'var(--surface-2)' : 'transparent',
            color: pathname === '/settings' ? 'var(--ink)' : 'var(--ink-3)',
          }}>
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
            <circle cx="8" cy="8" r="2.5"/>
            <path d="M8 1v1.5M8 13.5V15M1 8h1.5M13.5 8H15M3.05 3.05l1.06 1.06M11.89 11.89l1.06 1.06M3.05 12.95l1.06-1.06M11.89 4.11l1.06-1.06"/>
          </svg>
          <span className="text-[13px]">Settings</span>
          <span className="kbd ml-auto">6</span>
        </button>
      </div>

      {/* Active session pill */}
      {active && <ActiveSessionPill info={active} onClick={() => router.push('/today')} />}
    </aside>
  )
}
