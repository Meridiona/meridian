//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { AppGlyph } from 'meridian-design-system'

export function BrandIcon() {
  return <AppGlyph app="Google Chrome" size={28} />
}

export function LetterMonogram() {
  return <AppGlyph app="Terminal" size={28} />
}

export function WithName() {
  return <AppGlyph app="Visual Studio Code" size={24} withName />
}

export function Row() {
  return (
    <div style={{ display: 'flex', gap: 12, alignItems: 'center' }}>
      <AppGlyph app="Google Chrome" size={22} />
      <AppGlyph app="Claude" size={22} />
      <AppGlyph app="Slack" size={22} />
      <AppGlyph app="Figma" size={22} />
      <AppGlyph app={null} size={22} />
    </div>
  )
}
