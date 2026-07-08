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

/**
 * Pull just the "### TLDR" paragraph out of an activity-report markdown
 * string — used by the timeline row, which shows only the TLDR inline and
 * defers the full report (Core Tasks/Decisions/Resources) to the right-side
 * hour-detail panel on click. Falls back to the whole report if no TLDR
 * heading is found (defensive — the prompt always emits one).
 */
export function extractTldr(report: string): string {
  const match = report.match(/###\s*TLDR\s*\n+([\s\S]*?)(?=\n###|\s*$)/i)
  return match ? match[1].trim() : report.trim()
}

/**
 * The timeline row's inline preview of an hour's report — a small tinted
 * card (not raw ReactMarkdown output dropped straight into the row) with a
 * "SUMMARY" label, a 2-line-clamped TLDR, and a hover affordance hinting
 * that the row opens the full report (Core Tasks/Decisions/Resources) in
 * the right panel.
 */
export function TldrPreview({ report }: { report: string }) {
  const tldr = extractTldr(report)
  return (
    <div className="group rounded-md px-3 py-2.5 bg-box transition-colors">
      <div className="flex items-center justify-between gap-2 mb-1">
        <span className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>✦ Summary</span>
        <span className="mt-chip shrink-0 opacity-0 group-hover:opacity-100 transition-opacity"
          style={{ color: 'var(--color-state-proposal)' }}>
          Full report ›
        </span>
      </div>
      <p
        className="mt-body-sm"
        style={{
          color: 'var(--t-muted)',
          display: '-webkit-box',
          WebkitLineClamp: 2,
          WebkitBoxOrient: 'vertical',
          overflow: 'hidden',
        }}
      >
        {tldr}
      </p>
    </div>
  )
}

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
