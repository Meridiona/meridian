// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { ExternalLink } from 'lucide-react'
import { clsx } from 'clsx'
import * as Tooltip from '@radix-ui/react-tooltip'

interface TaskBadgeProps {
  taskKey: string | null
  sessionType: string | null         // 'task' | 'overhead' | 'unknown' | null
  routing: string | null             // 'auto' | 'queue' | 'skip'
  confidence: number | null
  method: string | null              // 'llm_standalone' | 'prefilter_trivial' | legacy: 'stage1_regex' | 'stage2_embed' | 'stage3_llm' | …
  taskTitle?: string | null
  taskUrl?: string | null
  size?: 'xs' | 'sm'
}

// ── Pipeline inference ────────────────────────────────────────────────────────
interface Stage {
  label: string
  deciding: boolean  // this stage produced the final answer
}

function inferPipeline(method: string | null): Stage[] {
  if (!method) return []
  switch (method) {
    // Current hermes pipeline methods
    case 'prefilter_trivial':
      return [{ label: 'Hermes · pre-filter', deciding: true }]
    case 'llm_standalone':
      return [{ label: 'Hermes · standalone', deciding: true }]
    // Legacy tagger pipeline methods (pre-existing DB rows)
    case 'stage1_regex':
      return [{ label: 'Stage 1 · regex', deciding: true }]
    case 'stage1_prefilter':
    case 'rule_prefilter':
      return [{ label: 'Stage 1 · pre-filter', deciding: true }]
    case 'stage2_embed':
    case 'semantic_embed':
      return [
        { label: 'Stage 1 · pre-filter', deciding: false },
        { label: 'Stage 2 · semantic', deciding: true },
      ]
    case 'stage3_llm':
    case 'stage3_llm_inspect':
    case 'agent_tiebreak':
      return [
        { label: 'Stage 1 · pre-filter', deciding: false },
        { label: 'Stage 2 · semantic', deciding: false },
        { label: 'Stage 3 · LLM', deciding: true },
      ]
    default:
      return [{ label: method, deciding: true }]
  }
}

// ── Tooltip pipeline component ────────────────────────────────────────────────
function PipelineTooltip({
  taskKey, taskTitle, sessionType, routing, confidence, method,
}: Pick<TaskBadgeProps, 'taskKey' | 'taskTitle' | 'sessionType' | 'routing' | 'confidence' | 'method'>) {
  const stages = inferPipeline(method)
  const pct = typeof confidence === 'number' && confidence > 0
    ? `${Math.round(confidence * 100)}%`
    : null

  const decidingResult = (() => {
    if (taskKey) return pct ? `${taskKey} · ${pct}` : taskKey
    if (sessionType === 'overhead') return 'overhead'
    if (routing === 'queue') return pct ? `queued · ${pct}` : 'queued'
    return pct ? `no match · ${pct}` : 'no match'
  })()

  return (
    <div className="space-y-2 min-w-[200px]">
      {/* Pipeline stages */}
      {stages.length > 0 ? (
        <div className="space-y-1">
          {stages.map((stage, i) => (
            <div key={i} className="flex items-center justify-between gap-4">
              <span className={clsx(
                'font-mono text-[10px] uppercase tracking-widest shrink-0',
                stage.deciding ? 'text-[#4B4A47]' : 'text-[#C8C6C1]',
              )}>
                {stage.label}
              </span>
              <span className={clsx(
                'text-[10px] font-mono',
                stage.deciding ? 'text-[#141414] font-medium' : 'text-[#C8C6C1]',
              )}>
                {stage.deciding ? decidingResult : '→ escalated'}
              </span>
            </div>
          ))}
        </div>
      ) : (
        <p className="text-[11px] text-[#9B9A97]">no stage data</p>
      )}

      {/* Task title if available */}
      {taskKey && taskTitle && (
        <>
          <div className="border-t border-[#F0EFEC]" />
          <p className="text-[11px] text-[#6B6A67] leading-snug max-w-[220px]">{taskTitle}</p>
        </>
      )}

      {/* Routing note for non-auto outcomes */}
      {routing && routing !== 'auto' && (
        <p className="text-[10px] text-[#C8C6C1]">
          routing: <span className="font-mono">{routing}</span>
        </p>
      )}
    </div>
  )
}

// ── Shared tooltip wrapper ────────────────────────────────────────────────────
function WithTooltip({ children, ...tooltipProps }: {
  children: React.ReactNode
} & React.ComponentPropsWithoutRef<typeof PipelineTooltip>) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>{children}</Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content
          side="top"
          align="center"
          sideOffset={5}
          className="z-50 rounded-lg border border-[#E8E6E1] bg-white px-3 py-2.5 shadow-md"
        >
          <PipelineTooltip {...tooltipProps} />
          <Tooltip.Arrow className="fill-white" />
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  )
}

// ── Main component ────────────────────────────────────────────────────────────
/**
 * Session classification badge with pipeline tooltip.
 *
 * Visual states:
 *   task + auto  + key   → blue pill "KAN-86 ↗"
 *   task + queue + key   → amber pill "KAN-86 ?"
 *   task + queue + nokey → amber pill "queued"
 *   task + skip  + nokey → muted pill "∅" (ran all stages, no match)
 *   overhead             → grey pill "overhead"
 *   null sessionType     → nothing (tagger hasn't processed this session)
 */
export default function TaskBadge({
  taskKey, sessionType, routing, confidence, method, taskTitle, taskUrl, size = 'xs',
}: TaskBadgeProps) {
  // Tagger hasn't run on this session yet
  if (!sessionType) return null

  const sizeClass = size === 'xs' ? 'text-[10px] px-1.5 py-0.5' : 'text-xs px-2 py-0.5'
  const tooltipProps = { taskKey, taskTitle: taskTitle ?? null, sessionType, routing, confidence, method }

  // ── Overhead ──────────────────────────────────────────────────────────────
  if (sessionType === 'overhead') {
    return (
      <WithTooltip {...tooltipProps}>
        <span className={clsx(
          'inline-flex items-center rounded-full font-medium cursor-default',
          'bg-[#F0EFEC] text-[#9B9A97]',
          sizeClass,
        )}>
          overhead
        </span>
      </WithTooltip>
    )
  }

  // ── Task: auto-assigned with a key ────────────────────────────────────────
  if (sessionType === 'task' && routing === 'auto' && taskKey) {
    const inner = (
      <>
        <span className="font-mono">{taskKey}</span>
        {taskUrl && <ExternalLink className="w-2.5 h-2.5 opacity-60" />}
      </>
    )
    const pill = taskUrl ? (
      <a
        href={taskUrl}
        target="_blank"
        rel="noopener noreferrer"
        className={clsx(
          'inline-flex items-center gap-1 rounded-full font-medium hover:brightness-95 transition',
          'bg-[#EBF0FB] text-[#3D5BB0] border border-[#D8E0F4]',
          sizeClass,
        )}
        onClick={(e) => e.stopPropagation()}
      >
        {inner}
      </a>
    ) : (
      <span className={clsx(
        'inline-flex items-center gap-1 rounded-full font-medium cursor-default',
        'bg-[#EBF0FB] text-[#3D5BB0] border border-[#D8E0F4]',
        sizeClass,
      )}>
        {inner}
      </span>
    )
    return <WithTooltip {...tooltipProps}>{pill}</WithTooltip>
  }

  // ── Task: queued for review ───────────────────────────────────────────────
  if (sessionType === 'task' && routing === 'queue') {
    return (
      <WithTooltip {...tooltipProps}>
        <span className={clsx(
          'inline-flex items-center gap-1 rounded-full font-medium cursor-default',
          'bg-[#FEF9EC] text-[#92400E] border border-[#FDE68A]',
          sizeClass,
        )}>
          <span className="font-mono">{taskKey ?? 'queued'}</span>
          <span className="opacity-60">?</span>
        </span>
      </WithTooltip>
    )
  }

  // ── Task: all stages ran but no ticket matched ────────────────────────────
  if (sessionType === 'task' && routing === 'skip') {
    return (
      <WithTooltip {...tooltipProps}>
        <span className={clsx(
          'inline-flex items-center rounded-full font-medium cursor-default',
          'bg-[#F8F7F4] text-[#C8C6C1] border border-[#F0EFEC]',
          sizeClass,
        )}>
          <span className="font-mono">∅</span>
        </span>
      </WithTooltip>
    )
  }

  return null
}
