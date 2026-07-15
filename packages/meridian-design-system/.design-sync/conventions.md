## Using meridian-design-system

**No provider or theme wrapper needed.** Every component reads its look from CSS custom properties on `:root` (shipped in `styles.css`) — there is no React context/theme provider to wrap your app in. Just import components from `meridian-design-system` and use them directly; `react`/`react-dom` are peer dependencies, resolve them from your own app.

### Styling idiom: Tailwind utilities + `var(--token)`, never hardcoded colors

Components mix two equivalent ways to apply a token — pick whichever reads better at the call site, both resolve to the same CSS custom property:

- **Tailwind utility classes** — `bg-<token>`, `text-<token>`, `border-<token>` (e.g. `className="bg-card border-card-border"`). These exist because `styles.css`'s `@theme` block maps `--color-<token>` Tailwind names onto the real custom properties.
- **Raw inline style** — `style={{ background: 'var(--t-card)', color: 'var(--ink-2)' }}`. Most components use this form for anything that isn't pure layout, especially state-driven colors (e.g. an accent bar keyed on approval state).

Real token names actually shipped (see `styles.css` for the full ~179-token set): surfaces — `card`, `panel`, `surface`, `surface-2`, `box`, `wrap`; text — `ink`, `ink-2`, `ink-3`, `ink-4`, `title`, `muted`, `faint`, `faint-2`; borders/rules — `rule`, `rule-2`, `hair`, `card-border`, `ctrl-border`, `input-border`; brand/state — `accent`, `accent-soft`, `success`, `warn`, `live`, `state-approved`, `state-pending`, `state-proposal`, `state-rejected`. Never hardcode a hex value for anything that has a token — every real component in this system reads color exclusively through one of the two forms above.

**Typography**: don't hand-roll `font-size`/`line-height`/`letter-spacing` combinations — use the semantic `mt-*` classes, each of which bundles a complete type treatment (family, size, leading, tracking, antialiasing): `mt-title`, `mt-title-lg`, `mt-modal-title`, `mt-card-title`, `mt-body`, `mt-body-sm`, `mt-label`, `mt-chip`, `mt-mono-sm`, `mt-stat`, `mt-greeting`, `mt-toolbar-date`. Pair with a text-color token (`style={{ color: 'var(--ink-2)' }}` or `className="text-ink-2"`) — these classes never bundle color.

**Fonts**: `--font-sans` (Plus Jakarta Sans, default body/UI) and `--font-mono` (JetBrains Mono, ticket keys/times/durations) — both already applied by the `mt-*` classes above, so you rarely set `font-family` directly.

### Where the truth lives

Read `styles.css` (the full token/type-scale source) before styling anything new. Per-component usage patterns and prop shapes are in each component's own `.prompt.md` / `.d.ts` in this bundle. `guidelines/STYLESHEET.md` (bundled) is the authoritative visual-system reference — exact type scale, color values per theme, spacing, radii, and shadows — cross-check against it for anything not covered above.

### Build snippet

```tsx
import { StatusPill } from 'meridian-design-system'

function ExampleRow({ item }) {
  return (
    <div className="p-4 flex items-center justify-between" style={{ background: 'var(--t-card)' }}>
      <span className="mt-body" style={{ color: 'var(--ink)' }}>{item.title}</span>
      <StatusPill status={item.state} isTerminal={item.isDone} />
    </div>
  )
}
```
