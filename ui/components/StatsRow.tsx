// meridian — AI activity intelligence by Meridiona

import { formatDuration } from '@/lib/format'
import type { StatsResponse } from '@/lib/types'
import AppIcon from './AppIcon'

interface StatsRowProps {
  stats: StatsResponse
}

function StatCell({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-[10px] uppercase tracking-widest text-[#9B9A97] font-medium">{label}</span>
      <span className="font-mono text-xl font-semibold text-[#141414] tabular-nums">{value}</span>
      {sub && <span className="text-xs text-[#C8C6C1]">{sub}</span>}
    </div>
  )
}

export default function StatsRow({ stats }: StatsRowProps) {
  const totalTrackedS = stats.focus_s + stats.user_idle_s + stats.away_s
  const focusPct = totalTrackedS > 0
    ? Math.round((stats.focus_s / totalTrackedS) * 100)
    : 0

  const awayS = stats.user_idle_s + stats.away_s

  const topApp = stats.top_apps[0]

  return (
    <div className="grid grid-cols-4 gap-px bg-[#E8E6E1] rounded-2xl overflow-hidden">
      {[
        {
          label: 'Focus',
          value: formatDuration(stats.focus_s),
          sub: `${focusPct}% of tracked time`,
        },
        {
          label: 'Away',
          value: formatDuration(awayS),
          sub: awayS > 0 ? [
            stats.user_idle_s > 0 ? `${formatDuration(stats.user_idle_s)} idle` : null,
            stats.away_s > 0 ? `${formatDuration(stats.away_s)} sleep` : null,
          ].filter(Boolean).join(' · ') || undefined : undefined,
        },
        {
          label: 'Sessions',
          value: String(stats.session_count),
        },
      ].map(cell => (
        <div key={cell.label} className="bg-white px-4 py-4">
          <StatCell {...cell} />
        </div>
      ))}

      <div className="bg-white px-4 py-4">
        <span className="text-[10px] uppercase tracking-widest text-[#9B9A97] font-medium block mb-1">
          Top App
        </span>
        {topApp ? (
          <div className="flex items-center gap-2">
            <AppIcon appName={topApp.app_name} size="sm" />
            <span className="text-sm font-medium text-[#141414] truncate">{topApp.app_name}</span>
          </div>
        ) : (
          <span className="font-mono text-lg text-[#C8C6C1]">—</span>
        )}
      </div>
    </div>
  )
}
