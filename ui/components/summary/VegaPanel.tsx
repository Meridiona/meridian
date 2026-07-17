//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Renders ONE model-chosen Vega-Lite spec.
//
// The spec arrives with its data bound BY NAME (`{"data": {"name": "segments"}}`)
// and no rows in it — the model chose the form, never the numbers. This injects the
// real rows at render, which is also why a stored chart can never drift from the
// timeline beside it (see migration 064).
//
// Theme: Vega defaults to a white card with its own type stack, which would read as
// a foreign object dropped onto the page. `vegaConfig` maps the app's own CSS tokens
// onto Vega's config so the charts belong here, in either theme.
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

/**
 * The app's tokens as a Vega config. Rebuilt per render key (the theme can change
 * under us), so charts re-skin with the rest of the page rather than staying light
 * on a dark ground.
 */
function vegaConfig() {
  const title = token('--t-title', '#211D3D')
  const muted = token('--t-muted', '#6E6A88')
  const hair = token('--t-hair', '#E4DDF7')
  const accent = token('--accent', '#7C3AED')
  // A categorical ramp anchored on the app's accent. Vega's default `tableau10`
  // is a perfectly good scheme that belongs to a different product.
  const scheme = [accent, '#10B981', '#F59E0B', '#EC4899', '#3B82F6', '#8B5CF6', '#14B8A6', '#F97316']
  const font = 'Geist, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif'

  return {
    background: 'transparent',
    font,
    padding: 4,
    axis: {
      labelColor: muted,
      titleColor: muted,
      labelFont: font,
      titleFont: font,
      labelFontSize: 10,
      titleFontSize: 10,
      titleFontWeight: 500 as const,
      titlePadding: 6,
      domainColor: hair,
      tickColor: hair,
      gridColor: hair,
      gridOpacity: 0.55,
      labelLimit: 140,
    },
    legend: {
      labelColor: muted,
      titleColor: muted,
      labelFont: font,
      titleFont: font,
      labelFontSize: 10,
      titleFontSize: 10,
      symbolType: 'circle' as const,
      labelLimit: 120,
    },
    title: { color: title, font, fontSize: 12, fontWeight: 600 as const, anchor: 'start' as const },
    view: { stroke: null },
    range: { category: scheme, ordinal: { scheme: 'purples' }, heatmap: { scheme: 'purples' } },
    mark: { color: accent },
    arc: { innerRadius: 0 },
    bar: { color: accent },
    line: { color: accent, strokeWidth: 2 },
    point: { color: accent, filled: true },
    area: { color: accent, opacity: 0.5 },
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
          renderer: 'canvas',
          // The page's own tooltip styling is out of scope; Vega's default is
          // legible and themed by the config above.
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
    <figure className="flex flex-col min-h-0 h-full rounded-2xl bg-card overflow-hidden"
      style={{ border: '1px solid var(--t-card-border)' }}>
      <figcaption className="px-3.5 pt-3 pb-1.5 shrink-0">
        <p className="mt-label truncate" style={{ color: 'var(--t-title)' }} title={panel.title}>
          {panel.title}
        </p>
        {panel.why && (
          <p className="mt-body-sm truncate" style={{ color: 'var(--t-faint-2)' }} title={panel.why}>
            {panel.why}
          </p>
        )}
      </figcaption>
      <div className="flex-1 min-h-0 px-2 pb-2">
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
