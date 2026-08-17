//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * "Meridian is open source" card, pinned to the bottom of the dashboard home
 * ([`OverviewPanel`]).
 *
 * # Why it lives at the BOTTOM of the overview
 * The home panel is what someone looks at while they work, so the top of it
 * belongs to their own day - the plan, time by app, time by category. An invite
 * to go read the source is worth showing every day but never worth outranking
 * any of that, and the overview already ends with the solo-mode tracker nudge,
 * so this reads as the last thing on the page rather than an interruption in
 * the middle of it.
 *
 * # Why every link here is checked to go somewhere
 * `ReportModal` carries the scar: Discord and X were listed before either
 * existed, so a screen whose whole promise was "a person reads this" shipped two
 * links to a dead invite and an empty profile. Every target below was verified
 * live before being added - the repo is public and MIT, and `good first issue`
 * is a real label with open issues behind it. If that label is ever emptied,
 * point the second action at [`CONTRIBUTING_URL`] rather than leaving a button
 * that lands on "0 results". "Star on GitHub" points at the plain repo URL -
 * GitHub has no direct "star" deep link, so the button's copy is the nudge and
 * the click lands them on the page with the star control visible.
 *
 * # Who calls this
 * [`OverviewPanel`] renders it unconditionally - there is no signed-out or
 * solo-mode variant, because the repo is public to everyone either way.
 *
 * # Related
 * - `ui/lib/bridge.ts` - `openExternal`, which routes through the Tauri opener
 *   command and falls back to `window.open` outside the app shell.
 * - `ui/components/timeline/ReportModal.tsx` - the other outbound-links surface.
 */
'use client'

import { openExternal } from '@/lib/bridge'

const REPO_URL = 'https://github.com/Meridiona/meridian'
/** Label-filtered issue list - verified non-empty before this shipped. */
const GOOD_FIRST_ISSUES_URL = `${REPO_URL}/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22`
/** Fallback target if `good first issue` is ever emptied - see the module doc. */
export const CONTRIBUTING_URL = `${REPO_URL}/blob/main/CONTRIBUTING.md`

/** Size in px, defaulted per icon - the only knob either of these takes. */
interface IconProps {
  size?: number
}

/** GitHub's mark, inline rather than from an icon pack so it inherits
 *  `currentColor` and stays correct in every theme (near-black on the light
 *  themes, near-white on ink) without a second asset to ship. */
function GithubMark({ size = 18 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
    </svg>
  )
}

/** Filled star, same `currentColor` treatment as {@link GithubMark}. */
function StarIcon({ size = 14 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <path d="M8 .5l2.13 4.62 5.02.58-3.75 3.42.98 5-4.38-2.55L3.62 14.1l.98-5L.85 5.7l5.02-.58L8 .5Z" />
    </svg>
  )
}

export function OpenSourceCard() {
  return (
    <div className="rounded-xl p-5 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
      <div className="flex items-center gap-2.5 mb-1.5">
        <span className="inline-flex items-center justify-center rounded-[9px] shrink-0"
          style={{ width: 30, height: 30, background: 'var(--t-wrap)', color: 'var(--t-title)' }}>
          <GithubMark />
        </span>
        <div className="flex-1 min-w-0">
          <p className="mt-card-title" style={{ color: 'var(--t-title)' }}>Meridian is open source</p>
          <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-faint)' }}>MIT licensed - the daemon, dashboard and tray</p>
        </div>
      </div>

      <p className="mt-body-sm" style={{ color: 'var(--t-muted)', marginTop: 10 }}>
        Browse the source, report anything that looks wrong, or send a pull request.
        Contributions are genuinely welcome.
      </p>

      <div className="flex flex-wrap" style={{ gap: 8, marginTop: 13 }}>
        <button type="button" onClick={() => openExternal(REPO_URL)}
          className="mt-card-hover flex items-center justify-center gap-1.5"
          style={{ flex: '1 1 130px', whiteSpace: 'nowrap', border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)', color: 'var(--t-title)', borderRadius: 12, padding: 11, font: "700 12.5px var(--font-sans)" }}>
          <StarIcon />
          <GithubMark size={14} />
          Star on GitHub ↗
        </button>
        <button type="button" onClick={() => openExternal(GOOD_FIRST_ISSUES_URL)}
          className="mt-card-hover text-center"
          style={{ flex: '1 1 130px', whiteSpace: 'nowrap', border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)', color: 'var(--t-title)', borderRadius: 12, padding: 11, font: "700 12.5px var(--font-sans)" }}>
          Good first issues ↗
        </button>
      </div>
    </div>
  )
}
