//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Barrel re-export of Meridian's shared UI primitives and a curated set of
// presentational timeline components, built standalone (esbuild, external
// react/radix/lucide) for Claude Design sync. Every export here is the real
// component from ui/components — this package never reimplements them.
// Excluded on purpose: components that fetch live app data at mount via
// @/lib/bridge's load()/mutate() (e.g. Toolbar, ThemeSwatches, most
// timeline/ modals/panels) — those throw outside the Tauri shell and have no
// standalone render. MeridianMark is the one piece pulled out of Toolbar.tsx
// (a pure-JSX logo mark; importing it doesn't execute Toolbar's own render).

// Form primitives
export { NumberStepper } from '@/components/ui/NumberStepper'
export { Select } from '@/components/ui/Select'
export { Switch } from '@/components/ui/Switch'
export { TextInput } from '@/components/ui/TextInput'

// Atoms — formatters, glyphs, badges, cards
export {
  fmtDur,
  fmtDurDecimal,
  fmtClock,
  CATS,
  PROVIDER_META,
  ProviderGlyph,
  CatDot,
  CatLabel,
  AppGlyph,
  shortTaskKey,
  TaskKey,
  StatusPill,
  LiveDot,
  SectionHead,
  Card,
  ConfidenceRing,
  SegBar,
  useTick,
} from '@/components/atoms'

export { ProviderIcon } from '@/components/ProviderIcon'

// Curated timeline pieces — presentational, props-driven, render standalone
export { CurvedArrow } from '@/components/timeline/CurvedArrow'
export { FloatingDraftsPill } from '@/components/timeline/FloatingDraftsPill'
export { ModalShell } from '@/components/timeline/ModalShell'
export { HourBadges, HourTakeover } from '@/components/timeline/HourBadges'
export { MeridianMark } from '@/components/timeline/Toolbar'
export { TimelineCard } from '@/components/timeline/TimelineCard'
export { TimeByApp } from '@/components/timeline/TimeByApp'
export { TimeByCategory } from '@/components/timeline/TimeByCategory'
export { EditableSummary, EditableTitle } from '@/components/timeline/EditableSummary'
