//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { ProviderGlyph, TaskKey } from '@/components/atoms'
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
        <div className="flex items-center gap-2 min-w-0">
          <ProviderGlyph provider={task.provider} size={16} />
          <TaskKey keyId={task.key} />
          {showType && <MetaChip>{task.issue_type}</MetaChip>}
          <span className="text-[13px] truncate"
            style={{ color: 'var(--ink)', textDecoration: task.is_terminal ? 'line-through' : 'none' }}>
            {task.title}
          </span>
        </div>

        {detail && task.description && (
          <p className="text-[12px] mt-1.5 leading-snug truncate" style={{ color: 'var(--ink-3)' }}>
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
            <span className="text-[10px] px-1.5 py-0.5 rounded-md" style={{ color: 'var(--success)', background: 'var(--surface-2)' }}>Done</span>
          )}
        </div>
      </div>
      {trail}
    </div>
  )
}
