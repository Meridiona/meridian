// meridian — AI activity intelligence by Meridiona

import { getCategoryMeta } from '@/lib/category-colors'

interface CategoryBadgeProps {
  category: string
  confidence?: number
  size?: 'xs' | 'sm'
}

export default function CategoryBadge({ category, confidence, size = 'sm' }: CategoryBadgeProps) {
  const meta = getCategoryMeta(category)
  const showConfidence = confidence !== undefined && confidence > 0
  const sizeClass = size === 'xs' ? 'text-[10px] px-1.5 py-0.5' : 'text-xs px-2 py-0.5'

  return (
    <span
      className={`inline-flex items-center gap-1 rounded-full font-medium ${sizeClass}`}
      style={{ backgroundColor: meta.bg, color: meta.color }}
    >
      <span aria-hidden>{meta.emoji}</span>
      {meta.label}
      {showConfidence && (
        <span style={{ opacity: 0.6 }} className="font-mono">
          {Math.round(confidence * 100)}%
        </span>
      )}
    </span>
  )
}
