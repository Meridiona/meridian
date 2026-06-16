//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useRef, useState } from 'react'
import { useRouter } from 'next/navigation'
import { TaskKey, StatusPill } from '@/components/atoms'

interface Props {
  onClose: () => void
}

interface CmdItem {
  kind: 'view' | 'task'
  label: string
  route: string
  key?: string
  status?: string
}

export default function CommandBar({ onClose }: Props) {
  const router = useRouter()
  const [q, setQ] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => { inputRef.current?.focus() }, [])

  const views: CmdItem[] = [
    { kind: 'view', label: 'Go to Today',    route: '/today' },
    { kind: 'view', label: "Go to Today's plan", route: '/plan' },
    { kind: 'view', label: 'Go to Tasks',    route: '/tasks' },
    { kind: 'view', label: 'Go to Worklogs', route: '/worklogs' },
    { kind: 'view', label: 'Go to Sessions', route: '/sessions' },
    { kind: 'view', label: 'Go to Week',     route: '/week' },
    { kind: 'view', label: 'Go to Clean-up', route: '/cleanup' },
  ]

  function navigate(item: CmdItem) {
    const dest = item.key ? `${item.route}?focus=${item.key}` : item.route
    router.push(dest)
    onClose()
  }

  const filtered = views.filter(a =>
    !q || a.label.toLowerCase().includes(q.toLowerCase())
  ).slice(0, 8)

  return (
    <div
      onClick={onClose}
      className="fixed inset-0 z-50 flex items-start justify-center pt-[18vh]"
      style={{ background: 'rgba(15,15,15,0.45)', backdropFilter: 'blur(4px)' }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="w-[560px] max-w-[92vw] rounded-xl overflow-hidden rise"
        style={{
          background: 'var(--surface)',
          border: '1px solid var(--rule-2)',
          boxShadow: '0 24px 80px rgba(0,0,0,0.18)',
        }}
      >
        <input
          ref={inputRef}
          value={q}
          onChange={e => setQ(e.target.value)}
          onKeyDown={e => {
            if (e.key === 'Escape') onClose()
            if (e.key === 'Enter' && filtered[0]) navigate(filtered[0])
          }}
          placeholder="Jump to view or task…"
          className="w-full px-5 py-4 text-[14px] font-sans"
          style={{
            background: 'transparent',
            color: 'var(--ink)',
            border: 'none',
            outline: 'none',
            borderBottom: '1px solid var(--rule)',
          }}
        />
        <div className="max-h-[40vh] overflow-y-auto nice-scroll">
          {filtered.length === 0 && (
            <p className="p-6 text-center text-[12px]" style={{ color: 'var(--ink-3)' }}>No matches.</p>
          )}
          {filtered.map((r, i) => (
            <button key={i}
              onClick={() => navigate(r)}
              className="w-full text-left px-5 py-2.5 flex items-center gap-3 transition-colors hover:opacity-80"
              style={{ background: i === 0 ? 'var(--surface-2)' : 'transparent' }}>
              {r.kind === 'task' && <TaskKey keyId={r.key} />}
              <span className="text-[13px]" style={{ color: 'var(--ink)' }}>{r.label}</span>
              {r.kind === 'task' && r.status && (
                <span className="ml-auto"><StatusPill status={r.status} /></span>
              )}
            </button>
          ))}
        </div>
        <div className="px-5 py-2.5 rule-t flex items-center justify-between text-[10px]"
          style={{ borderTopColor: 'var(--rule)', color: 'var(--ink-4)' }}>
          <span>Local search · no network</span>
          <span>
            <span className="kbd">↵</span> open · <span className="kbd">esc</span> close
          </span>
        </div>
      </div>
    </div>
  )
}
