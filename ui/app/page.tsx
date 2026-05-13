// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState, useCallback } from 'react'
import dynamic from 'next/dynamic'
import Sidebar from '@/components/Sidebar'
import CommandBar from '@/components/CommandBar'
import TweaksPanel from '@/components/TweaksPanel'

// Lazy-load heavy views
const TodayView    = dynamic(() => import('@/components/views/TodayView'),    { ssr: false })
const TasksView    = dynamic(() => import('@/components/views/TasksView'),    { ssr: false })
const QueueView    = dynamic(() => import('@/components/views/QueueView'),    { ssr: false })
const SessionsView = dynamic(() => import('@/components/views/SessionsView'), { ssr: false })
const WeekView     = dynamic(() => import('@/components/views/WeekView'),     { ssr: false })

type View = 'today' | 'tasks' | 'queue' | 'sessions' | 'week'

export default function DashboardPage() {
  const [view, setView] = useState<View>('today')
  const [focusKey, setFocusKey] = useState<string | null>(null)
  const [cmdOpen, setCmdOpen] = useState(false)
  const [queueCount, setQueueCount] = useState(0)

  const navigate = useCallback((v: View, key?: string) => {
    setView(v)
    setFocusKey(key ?? null)
    window.scrollTo({ top: 0, behavior: 'instant' })
  }, [])

  // fetch queue count for sidebar badge
  useEffect(() => {
    fetch('/api/queue-review')
      .then(r => r.json())
      .then(d => setQueueCount(d.items?.length ?? 0))
      .catch(() => {})
  }, [])

  // keyboard shortcuts
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
      if (e.key === '1') navigate('today')
      else if (e.key === '2') navigate('tasks')
      else if (e.key === '3') navigate('queue')
      else if (e.key === '4') navigate('sessions')
      else if (e.key === '5') navigate('week')
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [cmdOpen, navigate])

  const todayDate = new Date().toLocaleDateString('en-US', { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric' })

  return (
    <div className="min-h-screen flex" style={{ background: 'var(--paper)' }}>
      <Sidebar
        view={view}
        onNavigate={navigate}
        onOpenCmd={() => setCmdOpen(true)}
        queueCount={queueCount}
      />

      <main className="flex-1 min-w-0" data-screen-label={view}>
        <div className="max-w-[1080px] mx-auto px-10 py-14">
          {view === 'today'    && <TodayView    onNavigate={(v, k) => navigate(v as View, k)} />}
          {view === 'tasks'    && <TasksView    focusKey={focusKey} />}
          {view === 'queue'    && <QueueView    />}
          {view === 'sessions' && <SessionsView />}
          {view === 'week'     && <WeekView     />}

          <footer className="mt-24 pt-8 rule-t flex items-center justify-between text-[11px]"
            style={{ borderTopColor: 'var(--rule)', color: 'var(--ink-4)' }}>
            <span>Meridian · local · {todayDate}</span>
            <span className="font-mono tnum">
              <span className="kbd">⌘</span> <span className="kbd">K</span> to jump · <span className="kbd">1</span>–<span className="kbd">5</span> to switch view
            </span>
          </footer>
        </div>
      </main>

      {cmdOpen && (
        <CommandBar
          onClose={() => setCmdOpen(false)}
          onNavigate={(v, k) => { navigate(v, k); setCmdOpen(false) }}
        />
      )}

      <TweaksPanel />
    </div>
  )
}
