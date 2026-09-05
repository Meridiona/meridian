//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// "What's New" — release history + roadmap, opened via the toolbar nav
// pill's "What's New" item or auto-opened once per app version (see the
// tray's `poll::whats_new_auto_open`). Two tabs sharing one fetch: What's New
// (releases newest-first, from `get_whats_new`) and Roadmap. `scrollInside`
// gives the tab bar a fixed home while the content below scrolls on its own,
// same layout technique SettingsModal uses for its sidebar + content split.
//
// A release renders as at most three title + one-sentence entries, with no
// highlights/fixes split: the previous bulleted paragraphs were too long to
// actually get read, and the bucket a change came from is our concern, not
// the reader's. The brevity is enforced in `whats_new.rs`, not just curated.

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
  'in-progress': 'var(--t-accent)',
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
        <TabButton active={tab === 'whats-new'} onClick={() => setTab('whats-new')}>✨ What's New</TabButton>
        <TabButton active={tab === 'roadmap'} onClick={() => setTab('roadmap')}>🧭 Roadmap</TabButton>
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
        // Violet-tinted glow on the active tab — same accent the "Latest"
        // badge and release accent bar use, so the active state reads as
        // part of one system rather than a generic pressed-button shadow.
        boxShadow: active ? '0 8px 18px -8px rgba(139,92,246,.55)' : 'none',
        transition: 'box-shadow .2s, background .2s',
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
    <div className="flex flex-col gap-4">
      {releases.map((r, i) => {
        const latest = i === 0
        // Only the newest release gets the violet treatment — everything
        // lighting up equally would read as noise, not excitement.
        const accent = latest ? 'var(--t-accent)' : 'var(--t-card-border)'
        return (
          <div key={r.version} className="mer-pop rounded-2xl p-5 bg-card mt-card-hover"
            style={{ animationDelay: `${i * 60}ms`, boxShadow: `inset 3px 0 0 ${accent}` }}>
            <div className="flex items-center gap-2.5 mb-3">
              <p className="mt-card-title" style={{ color: latest ? accent : 'var(--t-title)' }}>v{r.version}</p>
              {latest && <LatestBadge />}
              <p className="mt-body-sm ml-auto" style={{ color: 'var(--t-faint)' }}>{r.date}</p>
            </div>
            <div className="flex flex-col gap-3">
              {/* Keyed on position within the release, not the title: two items
                  in one release may legitimately share a title, and a duplicate
                  key makes React reuse the wrong node. The list is static and
                  never reordered, so the index is a stable identity here. */}
              {r.items.map((item, idx) => (
                <div key={`${r.version}-${idx}`}>
                  <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontWeight: 600 }}>{item.title}</p>
                  <p className="mt-body-sm" style={{ color: 'var(--t-muted)', marginTop: 2 }}>{item.body}</p>
                </div>
              ))}
            </div>
          </div>
        )
      })}
    </div>
  )
}

function LatestBadge() {
  return (
    <span className="mer-pop mt-label" style={{
      padding: '3px 9px',
      borderRadius: 999,
      background: 'var(--color-state-proposal)',
      // Same fixed-dark-text rule as the roadmap status pills below — these
      // pill backgrounds are fixed brand colors, not theme tokens.
      color: 'rgba(0,0,0,0.78)',
    }}>
      ✨ Latest
    </span>
  )
}

function RoadmapList({ items }: { items: RoadmapItem[] }) {
  return (
    <div className="flex flex-col gap-3">
      {items.length === 0 ? (
        <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>Nothing on the roadmap yet.</p>
      ) : (
        items.map((item, i) => (
          <div key={item.title} className="mer-pop rounded-2xl p-4 mt-card-hover"
            style={{
              animationDelay: `${i * 60}ms`,
              border: '1px solid var(--t-card-border)',
              background: 'var(--t-box)',
              boxShadow: `inset 3px 0 0 ${ROADMAP_STATUS_COLOR[item.status]}`,
            }}>
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
        ))
      )}
      <RequestFeatureCta />
    </div>
  )
}

// Same channel the toolbar's Report modal offers ("Suggest a feature"), just
// surfaced in context here since a user reading the roadmap is the one most
// likely to have an opinion on what's missing from it.
function RequestFeatureCta() {
  return (
    <a href="mailto:hey@meridiona.com?subject=Feature%20suggestion"
      className="flex items-center justify-between gap-3 no-underline rounded-2xl mt-card-hover"
      style={{ padding: '13px 16px', border: '1px dashed var(--t-card-border)', background: 'var(--t-box)' }}>
      <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>Don&apos;t see what you're looking for?</p>
      <span className="mt-body-sm shrink-0" style={{ color: 'var(--btn-primary-bg)', fontWeight: 700 }}>
        Suggest a feature →
      </span>
    </a>
  )
}
