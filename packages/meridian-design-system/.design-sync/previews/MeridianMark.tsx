//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { MeridianMark } from 'meridian-design-system'

// MeridianMark's own source sets backgroundImage: url(/meridian-logo.png) —
// an absolute path only resolvable inside the real Next.js app (served from
// ui/public/). It can't load here, so the glyph itself renders as an
// invisible box (see NOTES.md). Kept as the real composition context (dark
// nav pill + label) since that's still a legitimate, useful preview of how
// it's used — not attempting to fake the missing icon.
export function InNavPill() {
  return (
    <div style={{ display: 'inline-flex', alignItems: 'center', gap: 6, background: '#1C1B2E', borderRadius: 999, padding: '4px 10px 4px 6px' }}>
      <MeridianMark size={15} />
      <span style={{ fontSize: 12, color: '#fff' }}>Meridian</span>
    </div>
  )
}
