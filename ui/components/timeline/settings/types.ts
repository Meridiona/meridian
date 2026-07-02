//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared types for the Settings surface — kept in their own file so callers
// (Toolbar's nav pill, MeridianTimelineShell, SettingsModal, the section
// files) can all reference `SettingsSection` without importing the modal
// component itself.

export type SettingsSection =
  | 'integrations'
  | 'capture'
  | 'notifications'
  | 'appearance'
  | 'advanced'
  | 'account'

export const DEFAULT_SETTINGS_SECTION: SettingsSection = 'integrations'
