---
name: meridian-ui
description: "Build, develop, and debug the Meridian Next.js dashboard. Covers component patterns, category system, API routes, and test runner."
allowed-tools: Bash, Read, Edit, Grep, Write
---

# Meridian UI Skill

## Stack

- **Next.js** (App Router) with React Server Components
- **TypeScript** — strict mode, no `any`
- **Tailwind CSS 4** — utility classes, no CSS modules
- **better-sqlite3** — synchronous SQLite, used in server components and API routes
- **bun:test** — test runner for `ui/__tests__/`

## Dev & Build

```bash
cd ui

# Development server (http://localhost:3000)
npm run dev

# Production build (must pass before committing)
npm run build

# Run UI tests
bun test
bun test --watch
```

## Key Files

| File | Purpose |
|------|---------|
| `ui/app/page.tsx` | Dashboard home — active session, stats, category breakdown, timeline |
| `ui/app/sessions/page.tsx` | Session list with pagination |
| `ui/app/apps/page.tsx` | Per-app stats with focus donut |
| `ui/app/api/active/route.ts` | Active session API |
| `ui/app/api/sessions/route.ts` | Sessions list API |
| `ui/app/api/stats/route.ts` | Daily stats API |
| `ui/app/api/timeline/route.ts` | Timeline segments API |
| `ui/lib/category-colors.ts` | 10-category palette, `getCategoryMeta()`, `CATEGORY_META` |
| `ui/lib/format.ts` | `formatDuration`, `formatDateLabel`, `toLocalDateString` |
| `ui/lib/types.ts` | Shared types: `SessionRow`, `ActiveSessionRow`, `StatsResponse` |
| `ui/components/CategoryBadge.tsx` | Pill badge with emoji + label + optional confidence |
| `ui/components/CategoryBreakdown.tsx` | Horizontal bar chart by category |
| `ui/components/DayTimeline.tsx` | Day timeline — segments colored by category |
| `ui/components/ActiveSessionCard.tsx` | Currently-active session card |
| `ui/components/SessionCard.tsx` | Completed session card |
| `ui/__tests__/` | Unit tests for format utilities and category system |

## Category System

10 fixed categories with a locked color palette:

```ts
import { getCategoryMeta, CATEGORY_META } from '@/lib/category-colors'

const meta = getCategoryMeta(session.category)
// meta.color  → '#4F7BE8'
// meta.bg     → '#EEF2FD'
// meta.emoji  → '💻'
// meta.label  → 'Coding'
```

Categories: `coding`, `code_review`, `meeting`, `communication`, `design`,
`documentation`, `planning`, `deployment_devops`, `research`, `idle_personal`.

Unknown strings fall back to `idle_personal`.

## DB Queries (better-sqlite3 pattern)

```ts
import getDb from '@/lib/db'

const db = getDb()
const rows = db.prepare(`
  SELECT id, app_name, category, confidence, duration_s
  FROM app_sessions
  WHERE started_at >= ? AND started_at < ?
  ORDER BY started_at DESC
`).all(start, end)
```

## Tests

```bash
# Run all UI tests
cd ui && bun test

# Run a specific file
bun test __tests__/format.test.ts
bun test __tests__/category-colors.test.ts
```

Tests live in `ui/__tests__/`. Use `bun:test` imports (`describe`, `it`, `expect`).

## Adding a New Component

1. Create `ui/components/MyComponent.tsx` with the file header:
   ```ts
   // meridian — normalises screenpipe activity into structured app sessions
   ```
2. Accept typed props — no `any`
3. Use `getCategoryMeta()` for any category coloring
4. Import from `@/lib/format` for duration/date formatting

## Common Issues

### `better-sqlite3` not found
```bash
cd ui && npm install
```

### Build type errors
```bash
cd ui && npm run build 2>&1 | head -50
```

### Category colors test fails
The palette is locked. If you change `CATEGORY_META` colors, update the hardcoded
assertions in `__tests__/category-colors.test.ts` to match.
