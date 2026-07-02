//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Renders an hour's activity-report markdown (the `/activity_report` LLM
// output — see services/agents/prompts/activity_report.py: ### section
// headings, **bold** topic/decision names, `-` bullets, plain paragraphs).
// Solo-mode only surface (see HourDetailPanel and TimelineColumn) — shared
// here so both the compact timeline row and the full hour-detail panel
// render the same markdown the same way, just at different type scale via
// `compact`.

'use client'

import ReactMarkdown from 'react-markdown'
import type { Components } from 'react-markdown'

export function ActivityReport({ report, compact = false }: { report: string; compact?: boolean }) {
  const bodyClass = compact ? 'mt-body-sm' : 'mt-body'
  const components: Components = {
    h1: ({ children }) => <ReportHeading>{children}</ReportHeading>,
    h2: ({ children }) => <ReportHeading>{children}</ReportHeading>,
    h3: ({ children }) => <ReportHeading>{children}</ReportHeading>,
    p: ({ children }) => (
      <p className={bodyClass} style={{ color: 'var(--t-muted)' }}>{children}</p>
    ),
    strong: ({ children }) => (
      <strong style={{ color: 'var(--t-title)', fontWeight: 700 }}>{children}</strong>
    ),
    ul: ({ children }) => <ul className="space-y-1.5 my-1" style={{ listStyle: 'none', paddingLeft: 0 }}>{children}</ul>,
    ol: ({ children }) => <ol className="space-y-1.5 my-1" style={{ listStyle: 'none', paddingLeft: 0 }}>{children}</ol>,
    // The prompt (services/agents/prompts/activity_report.py) only ever emits
    // unordered bullets ("- Key Decision", "- Resource"), so a single dot
    // marker — matching the rest of the design system's small muted markers
    // — covers every real report; no need to distinguish ordered lists.
    li: ({ children }) => (
      <li className={bodyClass} style={{ color: 'var(--t-muted)', display: 'flex', gap: 8 }}>
        <span aria-hidden="true" style={{ color: 'var(--t-faint)', flexShrink: 0 }}>·</span>
        <span className="min-w-0">{children}</span>
      </li>
    ),
    a: ({ href, children }) => (
      <a href={href} target="_blank" rel="noopener noreferrer" className="underline"
        style={{ color: 'var(--color-state-proposal)' }}>
        {children}
      </a>
    ),
  }

  return (
    <div className="space-y-2.5">
      <ReactMarkdown components={components}>{report}</ReactMarkdown>
    </div>
  )
}

function ReportHeading({ children }: { children: React.ReactNode }) {
  return (
    <p className="mt-label mt-4 first:mt-0" style={{ color: 'var(--t-faint)' }}>{children}</p>
  )
}
