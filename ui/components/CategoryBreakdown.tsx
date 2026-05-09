// meridian — AI activity intelligence by Meridiona

import { getCategoryMeta } from '@/lib/category-colors'
import { formatDuration } from '@/lib/format'

interface CategoryStat {
  category: string
  duration_s: number
}

interface CategoryBreakdownProps {
  stats: CategoryStat[]
}

export default function CategoryBreakdown({ stats }: CategoryBreakdownProps) {
  const top = [...stats]
    .filter(s => s.category !== 'idle_personal' && s.duration_s > 0)
    .sort((a, b) => b.duration_s - a.duration_s)
    .slice(0, 6)

  if (top.length === 0) return null

  const max = top[0].duration_s

  return (
    <div className="space-y-2">
      {top.map(s => {
        const meta = getCategoryMeta(s.category)
        const pct = max > 0 ? (s.duration_s / max) * 100 : 0
        return (
          <div key={s.category} className="flex items-center gap-3">
            <div className="w-24 shrink-0">
              <span className="text-xs text-[#6B6A67]">{meta.label}</span>
            </div>
            <div className="flex-1 h-2 rounded-full bg-[#E8E6E1] overflow-hidden">
              <div
                className="h-full rounded-full transition-all duration-500"
                style={{ width: `${pct}%`, backgroundColor: meta.color }}
              />
            </div>
            <span className="text-xs font-mono text-[#9B9A97] w-14 text-right tabular-nums">
              {formatDuration(s.duration_s)}
            </span>
          </div>
        )
      })}
    </div>
  )
}
