//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Renders ONE model-chosen Vega-Lite spec.
//
// The spec arrives with its data bound BY NAME (`{"data": {"name": "segments"}}`)
// and no rows in it — the model chose the form, never the numbers. This injects the
// real rows at render, which is also why a stored chart can never drift from the
// timeline beside it (see migration 064).
//
// Theme: Vega defaults to a white card with its own type stack, gridlines, axis
// rules and a blue categorical scheme — a Grafana panel dropped onto the page.
// `vegaConfig` maps the app's own CSS tokens onto Vega's config and strips the
// reporting furniture (grid, ticks, domain rules) so a chart reads as a picture of
// the day rather than a readout of it.
//
// Interaction: rendered as SVG with vega-embed's tooltip handler on, so every mark
// the model gave a `tooltip` encoding is hoverable. That is deliberate — it is what
// lets the exact numbers come off the axes and out of the chart body.
//
// Sizing: the spec is deliberately stripped of any width/height the model set — the
// grid owns layout, and a fixed size would break the one-screen rule. `autosize:fit`
// + a measured container is what makes a chart fill its tile.

'use client'

import { useEffect, useRef, useState } from 'react'
import type { SummaryPanel } from '@/lib/api-types'

/** Read a CSS custom property off the document, with a fallback for SSR/export. */
function token(name: string, fallback: string): string {
  if (typeof window === 'undefined') return fallback
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return v || fallback
}

/** `ink` is the app's dark palette (see lib/theme.ts) — the only one that wants
 *  Vega's dark tooltip. Read off the DOM rather than plumbed through as a prop:
 *  `applyTheme` writes the same attribute, so this cannot fall out of step. */
function isDark(): boolean {
  if (typeof document === 'undefined') return false
  return document.documentElement.dataset.theme === 'ink'
}

/**
 * The app's tokens as a Vega config. Rebuilt per render key (the theme can change
 * under us), so charts re-skin with the rest of the page rather than staying light
 * on a dark ground.
 */
function vegaConfig() {
  const title = token('--t-title', '#211D3D')
  const muted = token('--t-muted', '#6E6A88')
  const faint = token('--t-faint-2', '#8A85A6')
  const hair = token('--t-hair', '#E4DDF7')
  const accent = token('--accent', '#7C3AED')
  // A soft categorical ramp anchored on the app's accent. Vega's default
  // `tableau10` is a perfectly good scheme that belongs to a different product,
  // and its primary blue is what made the first version read as a BI dashboard.
  const scheme = [accent, '#EC4899', '#10B981', '#F59E0B', '#38BDF8', '#A78BFA', '#F472B6', '#2DD4BF']
  const font = 'Geist, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif'

  return {
    background: 'transparent',
    font,
    padding: 2,
    // NO GRID, no domain rule, no ticks. A gridline is an invitation to read a
    // value off an axis, which is a reporting move — this screen is for seeing a
    // shape, and the exact number is a hover away. Removing them is most of what
    // separates "a picture about your day" from "a panel in Grafana".
    axis: {
      labelColor: faint,
      titleColor: faint,
      labelFont: font,
      titleFont: font,
      labelFontSize: 10,
      titleFontSize: 10,
      titleFontWeight: 400 as const,
      titlePadding: 8,
      labelPadding: 6,
      domain: false,
      ticks: false,
      grid: false,
      labelLimit: 160,
    },
    legend: {
      labelColor: muted,
      titleColor: faint,
      labelFont: font,
      titleFont: font,
      labelFontSize: 10,
      titleFontSize: 10,
      symbolType: 'circle' as const,
      symbolSize: 60,
      labelLimit: 120,
      offset: 8,
    },
    title: { color: title, font, fontSize: 12, fontWeight: 600 as const, anchor: 'start' as const },
    view: { stroke: null },
    range: { category: scheme, ordinal: { scheme: 'purples' }, heatmap: { scheme: 'purples' } },
    mark: { color: accent },
    // Rounded, softened marks throughout: the same data drawn with square corners
    // and full opacity is the same data, and looks like a report of it.
    bar: { color: accent, cornerRadius: 3 },
    line: { color: accent, strokeWidth: 2.5, strokeCap: 'round' as const },
    point: { color: accent, filled: true, size: 70 },
    area: { color: accent, opacity: 0.35, line: { strokeWidth: 2 } },
    arc: { innerRadius: 54, padAngle: 0.02, cornerRadius: 3, stroke: hair, strokeWidth: 1 },
    rect: { cornerRadius: 3 },
  }
}

export function VegaPanel({
  panel,
  data,
  themeKey,
}: {
  panel: SummaryPanel
  /** name → rows, from `get_day_summary_data`. */
  data: Record<string, Record<string, unknown>[]>
  /** Changes when the theme does, forcing a re-embed with fresh tokens. */
  themeKey?: string
}) {
  const host = useRef<HTMLDivElement | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    let view: { finalize: () => void } | null = null
    const el = host.current
    if (!el) return

    // vega-embed pulls in the whole Vega runtime (~1MB). Imported dynamically so it
    // is fetched when a summary is actually opened, not on every dashboard boot.
    import('vega-embed')
      .then(({ default: embed }) => {
        if (cancelled || !host.current) return
        const spec = panel.spec as Record<string, unknown>
        const name = (spec?.data as { name?: string } | undefined)?.name
        const rows = name ? data[name] : undefined
        if (!name || !rows) {
          // Server-side validation should make this unreachable; if it happens,
          // say so rather than rendering an empty chart that looks like a real
          // answer of "nothing happened".
          setError('this chart is bound to data that is not on this day')
          return
        }

        const full = {
          ...spec,
          // The grid owns size. A model-set width/height would overflow the tile.
          width: 'container',
          height: 'container',
          autosize: { type: 'fit', contains: 'padding' },
          // Inject the rows the model was never given.
          data: { values: rows },
          config: vegaConfig(),
        }

        return embed(host.current, full as never, {
          actions: false,
          // SVG, not canvas. Canvas draws a flat bitmap: no hover target, no
          // crispness on a retina scale-up, and nothing for the tooltip handler to
          // bind to. The mark counts here are tiny (a day's segments, 24 hours), so
          // the usual canvas-for-volume argument does not apply.
          renderer: 'svg',
          // The interactivity. `tooltip: true` installs the default handler, which
          // reads whatever `tooltip` encoding the model wrote (the prompt asks for
          // one on every hoverable mark) — so a chart is something to explore, not
          // a picture to look at.
          tooltip: { theme: isDark() ? 'dark' : 'light' },
        }).then((res) => {
          if (cancelled) {
            res.view.finalize()
            return
          }
          view = res.view
          setError(null)
        })
      })
      .catch((e: unknown) => {
        if (cancelled) return
        // A spec can pass server validation and still fail here (a bad expression,
        // a scale it cannot resolve). One broken tile must not take the screen
        // down with it.
        setError(e instanceof Error ? e.message : 'this chart could not be drawn')
      })

    return () => {
      cancelled = true
      view?.finalize()
    }
  }, [panel, data, themeKey])

  return (
    <figure className="flex flex-col min-h-0 h-full rounded-2xl overflow-hidden px-4 pt-3.5 pb-3"
      style={{
        // A tint of the page rather than a bordered card. The first version framed
        // every chart in a hard-bordered white box, which is the single most
        // dashboard-like thing a chart can sit in.
        background: 'color-mix(in srgb, var(--t-card) 55%, transparent)',
        border: '1px solid color-mix(in srgb, var(--t-hair) 55%, transparent)',
      }}>
      {/* Title only. `why` is the model's reason for the form — worth demanding of
          it (asking makes it consider fit) and worth reading when curious, but a
          permanent second line of explanation under every chart is the clutter that
          made this read as a report. It lives on hover. */}
      <figcaption className="shrink-0 mb-2" title={panel.why || undefined}>
        <p className="mt-label truncate" style={{ color: 'var(--t-muted)' }}>
          {panel.title}
        </p>
      </figcaption>
      <div className="flex-1 min-h-0">
        {error ? (
          <div className="h-full flex items-center justify-center px-4">
            <p className="mt-body-sm text-center" style={{ color: 'var(--t-faint-2)' }}>{error}</p>
          </div>
        ) : (
          <div ref={host} className="w-full h-full" />
        )}
      </div>
    </figure>
  )
}
