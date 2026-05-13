// meridian — AI activity intelligence by Meridiona

'use client'

import { useState } from 'react'
import Link from 'next/link'
import * as Collapsible from '@radix-ui/react-collapsible'
import { ChevronDown, Mic, Clipboard, ArrowRight, Maximize2 } from 'lucide-react'
import AppIcon from './AppIcon'
import { formatDuration, formatTime } from '@/lib/format'
import type { SessionRow } from '@/lib/types'
import { clsx } from 'clsx'
import CategoryBadge from './CategoryBadge'
import TaskBadge from './TaskBadge'

interface SessionCardProps {
  session: SessionRow
}

export default function SessionCard({ session }: SessionCardProps) {
  const [open, setOpen] = useState(false)

  const hasDetail =
    session.window_titles.length > 1 ||
    (session.audio_snippets?.length ?? 0) > 0 ||
    (session.signals?.length ?? 0) > 0

  return (
    <Collapsible.Root open={open} onOpenChange={setOpen}>
      <div className={clsx(
        'rounded-xl border transition-colors',
        open ? 'border-[#D4D1CB]' : 'border-[#E8E6E1] hover:border-[#D4D1CB]',
        'bg-white overflow-hidden'
      )}>
        <Collapsible.Trigger asChild>
          <button
            className="w-full text-left px-4 py-3.5 flex items-center gap-3 group"
            disabled={!hasDetail}
          >
            <AppIcon appName={session.app_name} size="sm" />

            <div className="flex-1 min-w-0">
              <div className="flex items-center justify-between gap-2">
                <span className="text-sm font-medium truncate text-[#141414]">
                  {session.app_name}
                </span>
                <div className="flex items-center gap-1.5 shrink-0">
                  <span className="font-mono text-xs text-[#9B9A97] tabular-nums">
                    {formatDuration(session.duration_s)}
                  </span>
                  {session.category && (
                    <CategoryBadge category={session.category} size="xs" />
                  )}
                  <TaskBadge
                    taskKey={session.task_key}
                    sessionType={session.session_type}
                    routing={session.routing}
                    confidence={session.link_confidence}
                    method={session.link_method}
                    taskTitle={session.task_title}
                    taskUrl={session.task_url}
                    size="xs"
                  />
                </div>
              </div>

              <div className="flex items-center gap-2 mt-1">
                <span className="text-xs text-[#C8C6C1] font-mono tabular-nums">
                  {formatTime(session.started_at)}
                </span>
                {session.window_titles.slice(0, 1).map(w => (
                  <span
                    key={w.window_name}
                    className="text-xs text-[#9B9A97] truncate"
                  >
                    · {w.window_name}
                  </span>
                ))}
              </div>
            </div>

            {hasDetail && (
              <ChevronDown
                className={clsx(
                  'w-3.5 h-3.5 text-[#C8C6C1] shrink-0 transition-transform',
                  open && 'rotate-180'
                )}
              />
            )}
            <Link
              href={`/sessions/${session.id}`}
              onClick={(e) => e.stopPropagation()}
              className="text-[#C8C6C1] hover:text-[#6B6A67] transition-colors shrink-0"
              aria-label={`Open session ${session.id} detail`}
              title="Open detail"
            >
              <Maximize2 className="w-3.5 h-3.5" />
            </Link>
          </button>
        </Collapsible.Trigger>

        <Collapsible.Content className="overflow-hidden data-[state=closed]:animate-none">
          <div className="px-4 pb-4 pt-0 space-y-3 border-t border-[#E8E6E1]">

            {/* Windows */}
            {session.window_titles.length > 1 && (
              <div className="pt-3">
                <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-2">Windows</p>
                <div className="flex flex-wrap gap-1">
                  {session.window_titles.slice(0, 8).map(w => (
                    <span
                      key={w.window_name}
                      className="text-xs bg-[#F8F7F4] text-[#9B9A97] rounded px-2 py-0.5 truncate max-w-[200px]"
                    >
                      {w.window_name}
                      {w.count > 1 && (
                        <span className="ml-1 text-[#C8C6C1]">×{w.count}</span>
                      )}
                    </span>
                  ))}
                </div>
              </div>
            )}

            {/* Audio snippets */}
            {session.audio_snippets && session.audio_snippets.length > 0 && (
              <div className="pt-1">
                <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-2">Audio</p>
                <div className="space-y-1.5">
                  {session.audio_snippets.slice(0, 5).map((a, i) => (
                    <div key={i} className="flex items-start gap-2">
                      <Mic className="w-3 h-3 text-[#C8C6C1] mt-0.5 shrink-0" />
                      <p className="text-xs text-[#6B6A67] leading-relaxed">{a.transcription}</p>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Signals */}
            {session.signals && session.signals.length > 0 && (
              <div className="pt-1">
                <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-2">Signals</p>
                <div className="space-y-1.5">
                  {session.signals.slice(0, 5).map((s, i) => (
                    <div key={i} className="flex items-start gap-2">
                      {s.event_type === 'clipboard' ? (
                        <Clipboard className="w-3 h-3 text-[#C8C6C1] mt-0.5 shrink-0" />
                      ) : (
                        <ArrowRight className="w-3 h-3 text-[#C8C6C1] mt-0.5 shrink-0" />
                      )}
                      <p className="text-xs text-[#6B6A67] font-mono truncate">{s.value}</p>
                    </div>
                  ))}
                </div>
              </div>
            )}

          </div>
        </Collapsible.Content>
      </div>
    </Collapsible.Root>
  )
}
