//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Capture & Privacy → "Writing key". The switches for the Left Option writing key
// (`compose_*` in RuntimeSettings). It is on by default and reads on-screen text on every tap,
// so the master switch and an honest description of what is read and sent live here, next to
// the other "what leaves my machine" switches. macOS only: the card is not shown elsewhere.

'use client'

import { useState } from 'react'
import { Switch } from '@/components/ui/Switch'
import type { RuntimeSettings } from '@/lib/settings'
import { SectionCard, SectionHeader, FieldRow, SaveButton, type SaveStatus } from './fields'

/** True on macOS, the only platform the writing key exists on. The webview's user agent is
 *  the cheapest reliable signal; if it cannot be read, the card is hidden rather than
 *  offering switches that do nothing. */
function isMac(): boolean {
  if (typeof navigator === 'undefined') return false
  return /Mac/i.test(navigator.platform || navigator.userAgent || '')
}

export function WritingKeyCard({ settings, patch, save }: {
  settings: RuntimeSettings
  patch: (changes: Partial<RuntimeSettings>) => void
  save: (fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) => Promise<void>
}) {
  const [status, setStatus] = useState<SaveStatus>('idle')
  if (!isMac()) return null

  return (
    <SectionCard>
      <SectionHeader>Writing key</SectionHeader>
      <FieldRow label="Write with the Option key" description="Tap the left Option key in any text box and Meridian writes there: a reply, an opener, or a polish of what you typed. Each tap reads the text in the window you are in and sends it, with your Mac account name, to your AI provider to write the draft. A repeat tap within three minutes also sends the previous draft. Nothing else is sent - no history, no stored activity. On by default; turn it off here any time. Needs Accessibility and Input Monitoring access.">
        <Switch checked={settings.compose_enabled} onCheckedChange={v => patch({ compose_enabled: v })} />
      </FieldRow>
      <FieldRow label="Also read other open windows" description="Adds text from up to three other windows visible on screen, such as a document next to your chat, so a reply can use it. Password managers, banking and sign-in pages, private windows and anything on your capture ignore list are skipped.">
        <Switch checked={settings.compose_other_windows} onCheckedChange={v => patch({ compose_other_windows: v })} />
      </FieldRow>
      <FieldRow label="Typing dots" description="Types ... in the box while Meridian writes, like someone typing, and removes it before the draft appears.">
        <Switch checked={settings.compose_typing_dots} onCheckedChange={v => patch({ compose_typing_dots: v })} />
      </FieldRow>
      <FieldRow label="Typing sound" description="Plays a soft keyboard sound while Meridian writes.">
        <Switch checked={settings.compose_sound} onCheckedChange={v => patch({ compose_sound: v })} />
      </FieldRow>
      <SaveButton
        status={status}
        onClick={() => save({
          compose_enabled: settings.compose_enabled,
          compose_other_windows: settings.compose_other_windows,
          compose_typing_dots: settings.compose_typing_dots,
          compose_sound: settings.compose_sound,
        }, setStatus)}
      />
    </SectionCard>
  )
}
