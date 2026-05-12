// meridian — AI activity intelligence by Meridiona

import { ExternalLink } from 'lucide-react'
import { clsx } from 'clsx'

interface TaskBadgeProps {
  taskKey: string | null
  sessionType: string | null         // 'task' | 'overhead' | 'unknown'
  routing: string | null             // 'auto' | 'queue' | 'skip'
  confidence: number | null
  taskTitle?: string | null
  taskUrl?: string | null
  size?: 'xs' | 'sm'
}

/**
 * Pill rendered next to CategoryBadge on each session card.
 *
 *   • task_key set, routing in {auto, queue}        → indigo "KAN-86 ↗" pill, links to Jira when url present
 *   • session_type='overhead'                       → muted "overhead" pill
 *   • session_type='unknown' / link missing          → returns null (caller skips render)
 */
export default function TaskBadge({
  taskKey, sessionType, routing, confidence, taskTitle, taskUrl, size = 'xs',
}: TaskBadgeProps) {
  if (!sessionType) return null

  const sizeClass = size === 'xs' ? 'text-[10px] px-1.5 py-0.5' : 'text-xs px-2 py-0.5'

  // Overhead — muted grey pill, no link, no confidence.
  if (sessionType === 'overhead') {
    return (
      <span
        className={clsx(
          'inline-flex items-center rounded-full font-medium',
          'bg-[#F0EFEC] text-[#9B9A97]',
          sizeClass,
        )}
        title="Classified as overhead — not Jira-trackable"
      >
        overhead
      </span>
    )
  }

  // Unknown / no decision — render nothing rather than clutter.
  if (sessionType === 'unknown' || !taskKey) {
    return null
  }

  // Task with a real ticket key. Auto = saturated indigo, queue = muted indigo.
  const palette =
    routing === 'auto'
      ? 'bg-[#EBF0FB] text-[#3D5BB0] border border-[#D8E0F4]'
      : 'bg-[#F4F2EE] text-[#6B6A67] border border-[#E8E6E1]'

  const showConfidence = typeof confidence === 'number' && confidence > 0

  const inner = (
    <>
      <span className="font-mono">{taskKey}</span>
      {showConfidence && (
        <span className="font-mono opacity-60">
          {Math.round((confidence as number) * 100)}%
        </span>
      )}
      {taskUrl && <ExternalLink className="w-2.5 h-2.5 opacity-60" />}
    </>
  )

  const tooltip = [
    taskTitle ? `${taskKey}: ${taskTitle}` : taskKey,
    routing && `routing: ${routing}`,
    typeof confidence === 'number' && `confidence: ${confidence.toFixed(2)}`,
  ].filter(Boolean).join('\n')

  if (taskUrl) {
    return (
      <a
        href={taskUrl}
        target="_blank"
        rel="noopener noreferrer"
        title={tooltip}
        className={clsx(
          'inline-flex items-center gap-1 rounded-full font-medium hover:brightness-95 transition',
          palette,
          sizeClass,
        )}
        onClick={(e) => e.stopPropagation()}
      >
        {inner}
      </a>
    )
  }

  return (
    <span
      title={tooltip}
      className={clsx(
        'inline-flex items-center gap-1 rounded-full font-medium',
        palette,
        sizeClass,
      )}
    >
      {inner}
    </span>
  )
}
