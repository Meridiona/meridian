// meridian — AI activity intelligence by Meridiona

import { ExternalLink } from 'lucide-react'
import { formatDuration } from '@/lib/format'
import type { TicketBreakdownEntry } from '@/app/api/tickets/route'

interface TicketBreakdownProps {
  tasks: TicketBreakdownEntry[]
  overhead_s: number
  untagged_s: number
  /**
   * Cap rendered task rows. Overhead + Untagged always render below as
   * separate muted rows when nonzero.
   */
  topN?: number
}

export default function TicketBreakdown({
  tasks, overhead_s, untagged_s, topN = 8,
}: TicketBreakdownProps) {
  const top = tasks.slice(0, topN)

  // Bar scale uses the loudest single bucket (a task, or overhead/untagged
  // when nothing real was tagged) so the top entry always reaches 100%.
  const max = Math.max(
    top[0]?.duration_s ?? 0,
    overhead_s,
    untagged_s,
    1, // avoid divide-by-zero on an entirely empty day
  )

  if (top.length === 0 && overhead_s === 0 && untagged_s === 0) return null

  return (
    <div className="space-y-2">
      {top.map(t => {
        const pct = (t.duration_s / max) * 100
        const label = t.title ? `${t.task_key} · ${t.title}` : t.task_key
        const inner = (
          <>
            <div className="w-44 shrink-0 truncate flex items-center gap-1">
              <span className="font-mono text-[11px] text-[#3D5BB0]">{t.task_key}</span>
              {t.title && (
                <span className="text-xs text-[#6B6A67] truncate">· {t.title}</span>
              )}
              {t.url && <ExternalLink className="w-2.5 h-2.5 text-[#9B9A97] shrink-0" />}
            </div>
            <div className="flex-1 h-2 rounded-full bg-[#E8E6E1] overflow-hidden">
              <div
                className="h-full rounded-full transition-all duration-500"
                style={{ width: `${pct}%`, backgroundColor: '#3D5BB0' }}
              />
            </div>
            <span className="text-xs font-mono text-[#9B9A97] w-14 text-right tabular-nums">
              {formatDuration(t.duration_s)}
            </span>
          </>
        )
        if (t.url) {
          return (
            <a
              key={t.task_key}
              href={t.url}
              target="_blank"
              rel="noopener noreferrer"
              title={label}
              className="flex items-center gap-3 hover:bg-[#F8F7F4] -mx-2 px-2 py-1 rounded transition-colors"
            >
              {inner}
            </a>
          )
        }
        return (
          <div key={t.task_key} className="flex items-center gap-3 -mx-2 px-2 py-1">
            {inner}
          </div>
        )
      })}

      {(overhead_s > 0 || untagged_s > 0) && top.length > 0 && (
        <div className="border-t border-[#F0EFEC] pt-2 mt-2 space-y-2">
          {overhead_s > 0 && (
            <BreakdownRow label="overhead" duration_s={overhead_s} max={max} muted />
          )}
          {untagged_s > 0 && (
            <BreakdownRow label="untagged" duration_s={untagged_s} max={max} muted />
          )}
        </div>
      )}

      {top.length === 0 && (
        <>
          {overhead_s > 0 && (
            <BreakdownRow label="overhead" duration_s={overhead_s} max={max} muted />
          )}
          {untagged_s > 0 && (
            <BreakdownRow label="untagged" duration_s={untagged_s} max={max} muted />
          )}
        </>
      )}
    </div>
  )
}

function BreakdownRow({
  label, duration_s, max, muted = false,
}: { label: string; duration_s: number; max: number; muted?: boolean }) {
  const pct = (duration_s / max) * 100
  const bar = muted ? '#C8C6C1' : '#3D5BB0'
  const text = muted ? '#9B9A97' : '#6B6A67'
  return (
    <div className="flex items-center gap-3 -mx-2 px-2 py-1">
      <div className="w-44 shrink-0">
        <span className="text-xs" style={{ color: text }}>{label}</span>
      </div>
      <div className="flex-1 h-2 rounded-full bg-[#E8E6E1] overflow-hidden">
        <div
          className="h-full rounded-full transition-all duration-500"
          style={{ width: `${pct}%`, backgroundColor: bar }}
        />
      </div>
      <span className="text-xs font-mono text-[#9B9A97] w-14 text-right tabular-nums">
        {formatDuration(duration_s)}
      </span>
    </div>
  )
}
