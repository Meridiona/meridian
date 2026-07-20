//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The provider brand marks, in one place so the chooser tiles and the detail header show the
// SAME logo. Each is a simple monochrome glyph drawn with `currentColor`, so the caller sets
// the colour (neutral when idle, accent when the provider is in use) and both light and dark
// palettes are handled for free. These are recognisable, deliberately-simplified marks - not
// pixel-exact brand assets - kept lightweight and theme-aware rather than shipped as raster
// logos.

import type { LlmProviderId } from '@/lib/llm-providers'

/** Anthropic / Claude - the radial sunburst. */
function ClaudeMark({ s }: { s: number }) {
  return (
    <svg width={s} height={s} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      {Array.from({ length: 12 }).map((_, i) => (
        <rect
          key={i}
          x="11.2"
          y="2.4"
          width="1.6"
          height="7.2"
          rx="0.8"
          transform={`rotate(${i * 30} 12 12)`}
        />
      ))}
    </svg>
  )
}

/** OpenAI / Codex - a simplified interlocking rosette. */
function CodexMark({ s }: { s: number }) {
  return (
    <svg width={s} height={s} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {Array.from({ length: 6 }).map((_, i) => (
        <ellipse key={i} cx="12" cy="12" rx="3.4" ry="8" transform={`rotate(${i * 30} 12 12)`} />
      ))}
    </svg>
  )
}

/** Cursor - the isometric cube. */
function CursorMark({ s }: { s: number }) {
  return (
    <svg width={s} height={s} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 2.6 20.5 7v10L12 21.4 3.5 17V7z" />
      <path d="M12 2.6V12M12 12l8.5-5M12 12l-8.5-5M12 12v9.4" />
    </svg>
  )
}

/** Bring-your-own custom endpoint - a key. */
function KeyMark({ s }: { s: number }) {
  return (
    <svg width={s} height={s} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <circle cx="8" cy="8" r="4.2" />
      <path d="M11 11l7.5 7.5M16 16l2-2M14 18l2-2" />
    </svg>
  )
}

/**
 * The brand mark for one provider id. `custom` gets the key; an unknown id falls back to a
 * generic terminal glyph so the tile still renders.
 */
export function ProviderLogo({ id, size = 22 }: { id: LlmProviderId; size?: number }) {
  switch (id) {
    case 'claude':
      return <ClaudeMark s={size} />
    case 'codex':
      return <CodexMark s={size} />
    case 'cursor':
      return <CursorMark s={size} />
    case 'custom':
      return <KeyMark s={size} />
    default:
      return (
        <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
          strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <rect x="3" y="4" width="18" height="16" rx="2" /><path d="M7 9l3 3-3 3M13 15h4" />
        </svg>
      )
  }
}
