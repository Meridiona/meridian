//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { ProviderIcon } from '@/components/ProviderIcon'
import { DuePill, OriginChip, StatusChip, EpicChip, PriorityTag, MetaChip } from '@/components/plan/parts'

export interface CardTask {
  key: string
  title: string
  provider: string
  url: string
  due_days: number | null
  reason: string
  origin: string
  is_terminal?: boolean
  description?: string
  epic?: string | null
  status?: string
  priority?: string | null
  issue_type?: string
  story_points?: string | null
}

/** Presentational card body shared by the Today (compact) and board (detailed)
 *  columns. `lead` is the drag handle, `trail` the action controls; the central
 *  content opens the detail dialog via `onOpen`. When `detail` is set (the board)
 *  it also shows the description excerpt, priority and story points. */
export function TaskCardBody({
  task, lead, trail, detail = false, onOpen,
}: {
  task: CardTask
  lead?: React.ReactNode
  trail?: React.ReactNode
  detail?: boolean
  onOpen?: () => void
}) {
  const showType = task.issue_type && !/^task$/i.test(task.issue_type)
  // The origin chip only adds value for signals the status/due chips DON'T already
  // show — otherwise it duplicates them ("In progress" beside an "In Progress"
  // status, "Due 2d" beside the due pill). Keep it for carried-over / worked-recently.
  const showOrigin = (task.origin === 'carryover' || task.origin === 'recent') && !!task.reason

  return (
    <div className="px-3 py-2.5 flex items-start gap-2.5">
      {lead}
      <div
        role={onOpen ? 'button' : undefined} tabIndex={onOpen ? 0 : undefined}
        onClick={onOpen}
        onKeyDown={onOpen ? e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onOpen() } } : undefined}
        aria-label={onOpen ? `Open ${task.key} details` : undefined}
        className={`min-w-0 flex-1 rounded ${onOpen ? 'cursor-pointer' : ''}`}>
        <div className="flex items-center gap-1.5 min-w-0">
          {task.provider && <ProviderIcon provider={task.provider} size={13} className="shrink-0" />}
          <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text shrink-0">{task.key}</span>
          {showType && <MetaChip>{task.issue_type}</MetaChip>}
          <span className="mt-body-sm truncate"
            style={{ color: 'var(--t-title)', textDecoration: task.is_terminal ? 'line-through' : 'none' }}>
            {task.title}
          </span>
        </div>

        {detail && task.description && (
          <p className="mt-body-sm mt-1.5 leading-snug truncate" style={{ color: 'var(--t-faint)' }}>
            {task.description}
          </p>
        )}

        <div className="flex items-center gap-1.5 mt-1.5 flex-wrap">
          {showOrigin && <OriginChip reason={task.reason} origin={task.origin} />}
          <StatusChip status={task.status} />
          <EpicChip epic={task.epic} />
          <DuePill days={task.due_days} />
          {detail && <PriorityTag priority={task.priority} />}
          {detail && task.story_points && <MetaChip>{task.story_points} pts</MetaChip>}
          {task.is_terminal && (
            <span className="mt-chip px-1.5 py-0.5 rounded" style={{ color: 'var(--color-state-approved)', background: 'color-mix(in srgb, var(--color-state-approved) 12%, transparent)' }}>Done</span>
          )}
        </div>
      </div>
      {trail}
    </div>
  )
}
