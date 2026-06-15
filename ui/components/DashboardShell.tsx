//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState } from 'react'
import { useRouter, usePathname } from 'next/navigation'
import Sidebar from '@/components/Sidebar'
import CommandBar from '@/components/CommandBar'
import TweaksPanel from '@/components/TweaksPanel'
import HealthBanner from '@/components/HealthBanner'
import MustFixBanner from '@/components/MustFixBanner'

const KEY_ROUTES: Record<string, string> = {
  '1': '/today', '2': '/plan', '3': '/tasks',
  '4': '/worklogs', '5': '/sessions', '6': '/week',
  '7': '/cleanup', '8': '/settings',
}

export default function DashboardShell({ children }: { children: React.ReactNode }) {
  const router = useRouter()
  const pathname = usePathname()
  const [cmdOpen, setCmdOpen] = useState(false)

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

  // App-like pages bind to the viewport and scroll their own inner regions
  // (no page-level scrollbar, no footer). Document-flow pages keep the classic
  // scroll-the-whole-page layout. Keyed on route so non-app pages are unchanged.
  const appLike = pathname === '/plan'

  return (
    <div
      className={`flex flex-col ${appLike ? 'h-[100svh] overflow-hidden' : 'min-h-screen'}`}
      style={{ background: 'var(--paper)' }}
    >
      <HealthBanner />
      <MustFixBanner />
      <div className={`flex flex-1 ${appLike ? 'min-h-0' : ''}`}>
        <Sidebar onOpenCmd={() => setCmdOpen(true)} />
        <main
          className={`flex-1 min-w-0 ${appLike ? 'flex flex-col overflow-hidden' : ''}`}
          data-screen-label={pathname.slice(1)}
        >
          {appLike ? (
            <div className="flex-1 min-h-0 flex flex-col w-full max-w-[1080px] mx-auto px-10 py-10">
              {children}
            </div>
          ) : (
            <div className="max-w-[1080px] mx-auto px-10 py-14">
              {children}
              <footer
                className="mt-24 pt-8 rule-t flex items-center justify-between text-[11px]"
                style={{ borderTopColor: 'var(--rule)', color: 'var(--ink-4)' }}
              >
                <span>Meridian · local · {todayDate}</span>
                <span className="font-mono tnum">
                  <span className="kbd">⌘</span> <span className="kbd">K</span> to jump ·{' '}
                  <span className="kbd">1</span>–<span className="kbd">8</span> to switch view
                </span>
              </footer>
            </div>
          )}
        </main>
        {cmdOpen && (
          <CommandBar onClose={() => setCmdOpen(false)} />
        )}
        <TweaksPanel />
      </div>
    </div>
  )
}
