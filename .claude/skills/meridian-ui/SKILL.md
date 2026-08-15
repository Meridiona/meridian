---
name: meridian-ui
description: "Build, develop, and debug the Meridian dashboard - the Next.js static export that runs inside the Tauri tray webview. Covers the bridge to Rust, component patterns, and the bun test runner."
allowed-tools: Bash, Read, Edit, Grep, Write
---

# Meridian UI Skill

The dashboard is a **Next.js static export** (`output: 'export'` → `ui/out`) that runs
**inside the Tauri tray webview**. There is no Node server at runtime and there are no
`/api` route handlers - `ui/app/api/` does not exist, and `better-sqlite3` is not a
dependency of `ui/`. Everything that used to be a route handler is now Rust reached
over Tauri `invoke`.

If you are looking for the old route-handler / `@/lib/db` / `@/lib/category-colors`
world, it was removed in the Next fold. See `CLAUDE.md` → "Dashboard → Tauri fold".

## Stack

- **Next.js 16** (App Router), **React 19**
- **TypeScript** - strict, no `any` without a justifying comment
- **Tailwind CSS 4** - utility classes, no CSS modules
- **bun:test** - test runner for `ui/__tests__/`

## Dev & Build

```bash
cd ui

npm run dev        # dev server on :3939 (turbopack; NODE_ENV is unset deliberately)
npm run build      # production build + copies the tray popover into out/
npm run typecheck  # tsc --noEmit
bun test           # dashboard tests
```

Two gotchas worth knowing before you debug either:

- **Never run `next dev` with `NODE_ENV` set** - that is why the script is
  `env -u NODE_ENV next dev`. A stale `ui/.next` after a branch or config switch
  produces an `Invalid distDirRoot` panic; `rm -rf ui/.next` clears it.
- **The popover 404s under `tauri dev`** (next dev does not serve `popover/`). It
  renders correctly in a packaged build. This is expected, not a bug to chase.

## Reaching data: the bridge, never fetch

All data crosses to Rust through `ui/lib/bridge.ts`:

```ts
import { load, mutate, subscribe } from '@/lib/bridge'

const today = await load<TodayResponse>('/today', 'get_today')        // read
await mutate('/settings', 'save_settings', body, 'PATCH')             // write
const stop = subscribe<Health>('/health', 'get_health', 'health', cb) // live stream
```

- The first argument is a **vestigial** path that documents the former route; the
  second is the actual Tauri command name.
- The browser `fetch` and `EventSource` fallbacks were **removed at cutover** - these
  are Tauri-only.
- Response types live in **`ui/lib/api-types.ts`**.

Adding a new data source is a Rust job, not a TypeScript one: DB-backed reads go in
`meridian-core/src/readers/`, and file/env/process/HTTP work goes in
`tray/src-tauri/src/commands/`. Follow **CLAUDE.md → Coding Conventions → "Porting a
dashboard route to Rust"**, which covers placement, docs, tracing, and tests.

## Key files

| File | Purpose |
|------|---------|
| `ui/app/page.tsx` | Entry point; renders the timeline shell |
| `ui/app/setup/` | First-run wizard (its own window) |
| `ui/app/uninstall/` | Uninstall flow |
| `ui/lib/bridge.ts` | The only path to Rust - `load` / `mutate` / `subscribe` |
| `ui/lib/api-types.ts` | Response types for every command |
| `ui/lib/theme.ts`, `theme-context.tsx` | Surface palettes and theme plumbing |
| `ui/components/timeline/MeridianTimelineShell.tsx` | Top-level shell; owns modals, day state, and deep-link `navigate` |
| `ui/components/ConfirmDialog.tsx` | `ConfirmDialog` / `AlertDialog` - see the hard rule below |

## Hard rule: no native dialogs

**Never `window.confirm` / `window.alert` / `window.prompt`.** WKWebView routes JS
dialogs through a `WKUIDelegate` that nothing in the stack installs, so `confirm()`
always returns `false` and `alert()` is a silent no-op in the packaged tray. The damage
is silence - a falsy `confirm()` is indistinguishable from the user clicking Cancel, so
the gated action never runs while every log and test still passes. That is how the
Repair Database button shipped dead.

Use `@/components/ConfirmDialog` instead. `__tests__/no-native-dialogs.test.ts` fails
the build if they come back.

## Tests

```bash
cd ui
bun test                              # everything
bun test __tests__/deep-links.test.ts # one file
```

Tests live in `ui/__tests__/` and use `bun:test` imports (`describe`, `it`, `expect`).

Many are **source-scanning rather than behavioural** - they read the `.tsx` and assert
on what it contains. That is deliberate: it is the only available guard for things a
headless run cannot exercise (which element carries a ring, whether a `localStorage`
touch is wrapped, whether a deep link has a `navigate` arm). When adding one, assert
the invariant you actually mean, and confirm it fails against a deliberate regression -
a source scan that matches nothing passes vacuously.

## Adding a component

1. First line must be the file header:
   ```ts
   //ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
   ```
2. Typed props, no `any`.
3. Data through `@/lib/bridge`, types from `@/lib/api-types`.
4. User-facing strings use a plain hyphen `-`, never an em-dash - see the Hard Rules in
   `CLAUDE.md`.

## Common issues

### `Invalid distDirRoot` panic in dev
Stale cache after a branch or config switch: `rm -rf ui/.next`.

### An `invoke` silently does nothing
The window label is probably missing from `tray/src-tauri/capabilities/default.json`.
Un-permitted `invoke`s and events are denied without an error.

### Build type errors
```bash
cd ui && npm run typecheck
```
