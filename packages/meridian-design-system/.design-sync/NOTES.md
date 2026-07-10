# design-sync notes — meridian-design-system

## Repo-specific setup
- This package lives at `packages/meridian-design-system/` in the `meridian` monorepo, not at repo root. `.design-sync/` and `.ds-sync/` live inside the package dir (config home = the package), not the monorepo root.
- `dist/` is built by this package's own `build.mjs` (esbuild + tsc + a copy of the compiled Tailwind stylesheet/fonts from `ui/out`), NOT a generic `tsc`/`tsup` build. Run `cd ui && npm run build` first if you need the stylesheet/fonts to reflect current `ui/` source — `build.mjs` copies from `ui/out/_next/static/chunks`, so a stale `ui/out` means stale colors/fonts here.
- The converter's `--entry` is `./dist/index.js` (already a real esbuild-bundled ESM entry, not a synth-entry scan) — react/react-dom/@radix-ui/*/lucide-react are external in this dist build and resolve from this package's own `node_modules` at `--node-modules ./node_modules`.

## Known render limitations
- **`MeridianMark`**: its source (`ui/components/timeline/Toolbar.tsx`) hardcodes `backgroundImage: url(/meridian-logo.png)` — an absolute path only servable inside the real Next.js app (`ui/public/meridian-logo.png`). It cannot resolve in the design-sync preview environment, so the glyph itself renders as an invisible box (confirmed via screenshot). The kept "InNavPill" story still shows the real composition (dark pill + "Meridian" label) since that's a legitimate usage context — the icon itself just won't show. Not fixable without changing the source component (out of scope — this package never modifies `ui/`).

## Scope decisions
- **Excluded from the design system on purpose** (not bugs, deliberate v1 scope — see `packages/meridian-design-system/src/index.ts` header comment): `Toolbar`, `ThemeSwatches`, and most `timeline/` modals/panels — these fetch live app data at mount via `@/lib/bridge`'s `load()`/`mutate()`, which throws outside the Tauri shell. Only presentational, props-driven components were curated in.
- `CATS` and `PROVIDER_META` (plain data objects, capitalized but not React components) are excluded from the component scan via `componentSrcMap: {"CATS": null, "PROVIDER_META": null}` — the PascalCase heuristic would otherwise mistake them for components.
- All components ship under the `general` group (no docs/stories to categorize by) — regrouping into logical categories (Form/Atoms/Timeline) would need `docsMap` stub files per component; left as future polish, not required by the gate.

## Overrides applied (see config.json)
- `ModalShell`: `cardMode: "single"` + explicit viewport. Its own markup is `fixed inset-0` (a real full-viewport backdrop) — the preview wraps it in a `transform`-bearing container so the fixed element uses that as its CSS containing block instead of escaping to the true page viewport (which the capture harness measures as ~0 height). Composition-only fix; `ModalShell`'s source is untouched.
- `HourTakeover`: `cardMode: "column"` — it's designed to take over an entire hour row (wide horizontal card), not a small grid cell.
- `FloatingDraftsPill`: `cardMode: "single"` + viewport — its own markup is `position: absolute`, needs a sized positioned ancestor.
- `EditableSummary`, `TimeByCategory`, `TimelineCard`: `cardMode: "column"` — flagged by validate's `[GRID_OVERFLOW]` check (stories render wider than a standard grid cell).

## Re-sync risks
- The compiled stylesheet/fonts are a point-in-time copy from `ui/out` — if `ui/`'s Tailwind tokens or fonts change, re-run `cd ui && npm run build` before this package's `build.mjs`, or the design-system bundle will silently ship stale colors/fonts.
- `MeridianMark`'s known limitation above should be re-checked if `ui/components/timeline/Toolbar.tsx` ever changes how it resolves the brand-mark asset (e.g. if it's ever changed to accept an `iconUrl` prop, this could become fixable).
- The 7 `atoms.tsx` exports this package re-exports (`Card`, `ConfidenceRing`, `SegBar`, `LiveDot`, `CatLabel`, `fmtDurDecimal`, `useTick`) have zero consumers in the `ui/` dashboard app itself — flagged in code review as a merge-order conflict with a separate PR (#421) that deletes them as "dead code". If that PR merges, this package's `src/index.ts` barrel needs those 7 names dropped and a rebuild.
