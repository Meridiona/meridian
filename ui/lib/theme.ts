//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Applies one of the Meridian Timeline design's three theme palettes
// (lilac/blush/ink, defined in globals.css via html[data-theme]) to the
// document root. This replaces the old html.dark class mechanism from
// lib/theme-context.tsx for every surface under ui/components/timeline/**;
// theme-context.tsx itself is untouched (Sessions/Week/setup still use it).
//
// Persistence flows through RuntimeSettings.theme (get_settings/update_settings),
// not localStorage — callers read the current value from get_settings and call
// applyTheme() immediately (before the round-trip resolves) so there's no flash.

export type MeridianTheme = 'lilac' | 'blush' | 'ink'

export const THEME_IDS: MeridianTheme[] = ['lilac', 'blush', 'ink']

export function applyTheme(theme: MeridianTheme): void {
  if (typeof document === 'undefined') return
  document.documentElement.dataset.theme = theme
}
