// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import { useRouter, usePathname } from 'next/navigation'
import Sidebar from '@/components/Sidebar'
import CommandBar from '@/components/CommandBar'
import TweaksPanel from '@/components/TweaksPanel'
import HealthBanner from '@/components/HealthBanner'

const KEY_ROUTES: Record<string, string> = {
  '1': '/today', '2': '/tasks', '3': '/queue',
  '4': '/worklogs', '5': '/sessions', '6': '/week', '7': '/settings',
}

export default function DashboardShell({ children }: { children: React.ReactNode }) {
  const router = useRouter()
  const pathname = usePathname()
  const [cmdOpen, setCmdOpen] = useState(false)
  const [queueCount, setQueueCount] = useState(0)

  useEffect(() => {
    fetch('/api/queue-review')
      .then(r => r.json())
      .then(d => setQueueCount(d.items?.length ?? 0))
      .catch(() => {})
  }, [])

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setCmdOpen(o => !o)
        return
      }
      if (e.key === 'Escape') { setCmdOpen(false); return }
      if (cmdOpen || e.metaKey || e.ctrlKey || e.altKey) return
      const target = e.target as HTMLElement
      if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return
      const route = KEY_ROUTES[e.key]
      if (route) router.push(route)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [cmdOpen, router])

  const todayDate = new Date().toLocaleDateString('en-US', {
    weekday: 'long', month: 'long', day: 'numeric', year: 'numeric',
  })

  return (
    <div className="min-h-screen flex flex-col" style={{ background: 'var(--paper)' }}>
      <HealthBanner />
      <div className="flex flex-1">
        <Sidebar onOpenCmd={() => setCmdOpen(true)} queueCount={queueCount} />
        <main className="flex-1 min-w-0" data-screen-label={pathname.slice(1)}>
          <div className="max-w-[1080px] mx-auto px-10 py-14">
            {children}
            <footer
              className="mt-24 pt-8 rule-t flex items-center justify-between text-[11px]"
              style={{ borderTopColor: 'var(--rule)', color: 'var(--ink-4)' }}
            >
              <span>Meridian · local · {todayDate}</span>
              <span className="font-mono tnum">
                <span className="kbd">⌘</span> <span className="kbd">K</span> to jump ·{' '}
                <span className="kbd">1</span>–<span className="kbd">7</span> to switch view
              </span>
            </footer>
          </div>
        </main>
        {cmdOpen && (
          <CommandBar onClose={() => setCmdOpen(false)} />
        )}
        <TweaksPanel />
      </div>
    </div>
  )
}
