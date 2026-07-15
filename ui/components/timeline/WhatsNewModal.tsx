//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// "What's New" — release history + roadmap, opened via the toolbar nav
// pill's "What's New" item or auto-opened once per app version (see the
// tray's `poll::whats_new_auto_open`). Two tabs sharing one fetch: What's New
// (releases newest-first, from `get_whats_new`) and Roadmap. `scrollInside`
// gives the tab bar a fixed home while the content below scrolls on its own,
// same layout technique SettingsModal uses for its sidebar + content split.

'use client'

import { useEffect, useState } from 'react'
import { load } from '@/lib/bridge'
import type { ReleaseNote, RoadmapItem, WhatsNewData } from '@/lib/api-types'
import { ModalShell } from './ModalShell'

type Tab = 'whats-new' | 'roadmap'

const ROADMAP_STATUS_LABEL: Record<RoadmapItem['status'], string> = {
  'in-progress': 'In progress',
  planned: 'Planned',
  considering: 'Considering',
}

const ROADMAP_STATUS_COLOR: Record<RoadmapItem['status'], string> = {
  'in-progress': 'var(--color-state-proposal)',
  planned: 'var(--color-state-pending)',
  considering: 'var(--color-state-rejected)',
}

export function WhatsNewModal({ onClose }: { onClose: () => void }) {
  const [tab, setTab] = useState<Tab>('whats-new')
  const [data, setData] = useState<WhatsNewData | null>(null)

  useEffect(() => {
    load<WhatsNewData>('/whats-new', 'get_whats_new')
      .then(setData)
      .catch(() => setData({ releases: [], roadmap: [] }))
  }, [])

  return (
    <ModalShell title="What's New" onClose={onClose} maxWidth={640} scrollInside>
      <div className="flex gap-2 px-7 pt-5 pb-4 shrink-0 border-b" style={{ borderColor: 'var(--t-hair)' }}>
        <TabButton active={tab === 'whats-new'} onClick={() => setTab('whats-new')}>What's New</TabButton>
        <TabButton active={tab === 'roadmap'} onClick={() => setTab('roadmap')}>Roadmap</TabButton>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto nice-scroll px-7 py-6">
        {!data ? (
          <div className="flex flex-col gap-3">
            {[1, 2].map(i => (
              <div key={i} className="rounded-2xl h-24 bg-card" style={{ opacity: 0.5 }} />
            ))}
          </div>
        ) : tab === 'whats-new' ? (
          <ReleaseList releases={data.releases} />
        ) : (
          <RoadmapList items={data.roadmap} />
        )}
      </div>
    </ModalShell>
  )
}

function TabButton({ active, onClick, children }: {
  active: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button onClick={onClick} className="mt-body-sm"
      style={{
        padding: '7px 14px',
        borderRadius: 999,
        border: '1px solid var(--t-ctrl-border)',
        background: active ? 'var(--btn-primary-bg)' : 'var(--t-ctrl)',
        color: active ? '#fff' : 'var(--t-muted)',
        cursor: 'pointer',
      }}>
      {children}
    </button>
  )
}

function ReleaseList({ releases }: { releases: ReleaseNote[] }) {
  if (releases.length === 0) {
    return <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>No release notes yet.</p>
  }
  return (
    <div className="flex flex-col gap-6">
      {releases.map(r => (
        <div key={r.version}>
          <div className="flex items-baseline gap-2.5 mb-2.5">
            <p className="mt-card-title" style={{ color: 'var(--t-title)' }}>v{r.version}</p>
            <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>{r.date}</p>
          </div>
          {r.highlights.length > 0 && (
            <BulletList items={r.highlights} />
          )}
          {r.fixes.length > 0 && (
            <>
              <p className="mt-label mt-3 mb-1.5" style={{ color: 'var(--t-faint)' }}>FIXES</p>
              <BulletList items={r.fixes} />
            </>
          )}
        </div>
      ))}
    </div>
  )
}

function BulletList({ items }: { items: string[] }) {
  return (
    <ul className="flex flex-col gap-1.5">
      {items.map((item, i) => (
        <li key={i} className="mt-body-sm flex gap-2" style={{ color: 'var(--t-muted)' }}>
          <span aria-hidden="true" style={{ color: 'var(--t-faint-2)' }}>•</span>
          <span>{item}</span>
        </li>
      ))}
    </ul>
  )
}

function RoadmapList({ items }: { items: RoadmapItem[] }) {
  if (items.length === 0) {
    return <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>Nothing on the roadmap yet.</p>
  }
  return (
    <div className="flex flex-col gap-3">
      {items.map(item => (
        <div key={item.title} className="rounded-2xl p-4"
          style={{ border: '1px solid var(--t-card-border)', background: 'var(--t-box)' }}>
          <div className="flex items-center justify-between gap-2 mb-1.5">
            <p className="mt-card-title" style={{ color: 'var(--t-title)' }}>{item.title}</p>
            <span className="mt-label shrink-0" style={{
              padding: '3px 9px',
              borderRadius: 999,
              background: ROADMAP_STATUS_COLOR[item.status],
              // Fixed dark text, not a theme token: these pill backgrounds are
              // fixed brand colors (not theme-derived), and white text fails
              // contrast against the lighter ones (e.g. the amber "Planned" /
              // lavender "Considering" pills) — dark text reads cleanly
              // against all three regardless of theme.
              color: 'rgba(0,0,0,0.78)',
            }}>
              {ROADMAP_STATUS_LABEL[item.status]}
            </span>
          </div>
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>{item.description}</p>
        </div>
      ))}
    </div>
  )
}
