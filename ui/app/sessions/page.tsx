// meridian — AI activity intelligence by Meridiona

'use client'

import { useState, useEffect, useCallback } from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import SessionCard from '@/components/SessionCard'
import { formatDateLabel, toLocalDateString } from '@/lib/format'
import type { PaginatedSessions, SessionRow } from '@/lib/types'

export default function SessionsPage() {
  const [date, setDate] = useState(toLocalDateString())
  const [sessions, setSessions] = useState<SessionRow[]>([])
  const [total, setTotal] = useState(0)
  const [hasMore, setHasMore] = useState(false)
  const [page, setPage] = useState(1)
  const [loading, setLoading] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)

  const loadPage = useCallback(async (d: string, p: number, append: boolean) => {
    if (p === 1) {
      setLoading(true)
    } else {
      setLoadingMore(true)
    }
    try {
      const res = await fetch(`/api/sessions?date=${d}&page=${p}&page_size=30`)
      const json: PaginatedSessions = await res.json()
      if (append) {
        setSessions(prev => [...prev, ...json.sessions])
      } else {
        setSessions(json.sessions)
      }
      setTotal(json.total)
      setHasMore(json.has_more)
    } finally {
      setLoading(false)
      setLoadingMore(false)
    }
  }, [])

  // When date changes, reset to page 1
  useEffect(() => {
    setPage(1)
    setSessions([])
    setHasMore(false)
    loadPage(date, 1, false)
  }, [date, loadPage])

  function offsetDate(days: number) {
    const d = new Date(date + 'T12:00:00')
    d.setDate(d.getDate() + days)
    setDate(toLocalDateString(d))
  }

  function handleLoadMore() {
    const nextPage = page + 1
    setPage(nextPage)
    loadPage(date, nextPage, true)
  }

  return (
    <div className="space-y-5">
      {/* Header + date nav */}
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">Sessions</h1>
        <div className="flex items-center gap-2">
          <button
            onClick={() => offsetDate(-1)}
            className="w-8 h-8 rounded-lg flex items-center justify-center hover:bg-[#E8E6E1] transition-colors"
          >
            <ChevronLeft className="w-4 h-4 text-[#9B9A97]" />
          </button>
          <input
            type="date"
            value={date}
            onChange={e => setDate(e.target.value)}
            className="border border-[#E8E6E1] rounded-lg px-3 py-1.5 text-sm font-mono text-[#141414] bg-white focus:outline-none focus:border-[#141414] transition-colors"
          />
          <button
            onClick={() => offsetDate(1)}
            className="w-8 h-8 rounded-lg flex items-center justify-center hover:bg-[#E8E6E1] transition-colors"
            disabled={date >= toLocalDateString()}
          >
            <ChevronRight className="w-4 h-4 text-[#9B9A97]" />
          </button>
        </div>
      </div>

      <div className="flex items-center justify-between">
        <span className="text-[#9B9A97] text-sm">{formatDateLabel(date)}</span>
        {!loading && sessions.length > 0 && (
          <span className="text-xs text-[#C8C6C1] font-mono">
            {sessions.length} of {total} sessions
          </span>
        )}
      </div>

      {loading && (
        <div className="space-y-2">
          {[1, 2, 3].map(i => (
            <div key={i} className="h-16 rounded-xl bg-[#E8E6E1] animate-pulse" />
          ))}
        </div>
      )}

      {!loading && sessions.length === 0 && (
        <div className="rounded-xl border border-[#E8E6E1] bg-white px-5 py-12 text-center">
          <p className="text-sm text-[#9B9A97]">No sessions on {formatDateLabel(date)}</p>
        </div>
      )}

      {!loading && sessions.length > 0 && (
        <div className="space-y-2">
          {sessions.map(s => <SessionCard key={s.id} session={s} />)}
        </div>
      )}

      {hasMore && !loading && (
        <button
          onClick={handleLoadMore}
          disabled={loadingMore}
          className="w-full py-2.5 text-sm text-[#9B9A97] border border-[#E8E6E1] rounded-xl hover:border-[#D4D1CB] hover:text-[#141414] transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {loadingMore ? 'Loading...' : 'Load more'}
        </button>
      )}
    </div>
  )
}
