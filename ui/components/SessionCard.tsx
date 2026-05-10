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

function roleBadgeClass(role: string | null): string {
  if (!role) return 'bg-[#F0EFEC] text-[#9B9A97]'
  if (role.includes('Button')) return 'bg-[#EBF0FB] text-[#4A6CC4]'
  if (role.includes('Heading')) return 'bg-[#FBF0EB] text-[#C47A4A]'
  if (role.includes('Link')) return 'bg-[#EBF8F0] text-[#4A9E6A]'
  return 'bg-[#F0EFEC] text-[#9B9A97]'
}

export default function SessionCard({ session }: SessionCardProps) {
  const [open, setOpen] = useState(false)

  const hasDetail =
    session.window_titles.length > 1 ||
    (session.ocr_samples?.length ?? 0) > 0 ||
    (session.elements_samples?.length ?? 0) > 0 ||
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

            {/* Screen Text (OCR) */}
            {session.ocr_samples && session.ocr_samples.length > 0 && (
              <div className="pt-1">
                <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-2">Screen Text</p>
                <div className="space-y-2">
                  {session.ocr_samples.slice(0, 5).map((o, i) => (
                    <div key={i}>
                      {o.window_name && (
                        <p className="text-[10px] text-[#C8C6C1] mb-0.5">· {o.window_name}</p>
                      )}
                      <p className="text-xs text-[#6B6A67] leading-relaxed line-clamp-3 whitespace-pre-wrap">
                        {o.text}
                      </p>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* UI Elements (Accessibility) */}
            {session.elements_samples && session.elements_samples.length > 0 && (
              <div className="pt-1">
                <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-2">UI Elements</p>
                <div className="flex flex-wrap gap-1.5">
                  {session.elements_samples.slice(0, 8).map((el, i) => (
                    <span
                      key={i}
                      className={clsx(
                        'inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium',
                        roleBadgeClass(el.role)
                      )}
                    >
                      {el.role && (
                        <span className="opacity-60">{el.role.replace(/^AX/, '')}</span>
                      )}
                      <span className="truncate max-w-[120px]">{el.text}</span>
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
