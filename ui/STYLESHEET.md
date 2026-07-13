//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

# Meridian Timeline — Style Sheet

The complete visual system for the Meridian daily-timeline desktop app: type, color tokens across all three themes, spacing, radii, shadows, and every component's exact values. All styling is applied inline in the source mock (no CSS classes); in our implementation the equivalent values live in `ui/app/globals.css` (`.mt-*` type classes + `--t-*`/`--color-*` tokens) and, for the tray popover, `tray/src/style.css` (an independent, single-theme token block — see that file's header comment). This document is the source of truth for what those must match. Fixed canvas in the mock: 1280 × 840px (our shell is fluid — see `MeridianTimelineShell.tsx`).

## 1 · Typography

One voice: SF Pro for everything — UI text and numerics alike. `--font-sans`
resolves via `-apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui` — on
macOS (Meridian's actual runtime) this renders the real SF Pro Text/Display
already installed on the system, no font files bundled. There is no separate
monospace face for ticket keys/times/durations; numeric alignment comes from
`font-variant-numeric: tabular-nums` on the same family (the `.font-mono` /
`.mt-mono-sm` classes in `globals.css` and `tray/src/style.css`), matching how
Apple's own type system handles numeric columns — no dedicated mono face there
either. (`Instrument_Serif` remains, unrelated — it's a legacy face used only by
the setup wizard, not part of this type scale.)

| Role | Size | Weight | Line / Tracking |
|---|---|---|---|
| Panel heading ("You had a solid day") | 20px | 800 | 1.25 / −.02em |
| Modal title (Review, Cleanup, Plan) | 17–22px | 800 | 1.2 / −.02em |
| Swipe-card ticket title | 19px | 800 | 1.3 / −.02em |
| Toolbar date | 15px | 700 | 1.1 |
| Worklog card title | 13.5px | 700 | 1.3 / −.01em |
| Body / summary text | 12–14px | 500 | 1.45–1.55 |
| Section label (TODAY'S FOCUS) | 11px | 700 | / .08em, UPPER |
| Ticket key (MER-482) | 11–12px | 600–700 | / .02em, tabular-nums |
| Hour label / time / duration | 10.5–11px | 600 | / .02em, tabular-nums |
| Status pill (NEEDS REVIEW) | 9.5px | 700 | / .03em, UPPER |

Global smoothing: `-webkit-font-smoothing: antialiased`. Base reset: `*{box-sizing:border-box}`, `html,body{margin:0;padding:0}`.

Non-Mac caveat (relevant once a website/marketing surface exists): `-apple-system`/
`system-ui` falls back to each OS's native UI font off Apple hardware (Segoe UI,
Roboto, …), not true SF Pro — Apple doesn't distribute the font for that use.
Accepted for now; Inter is the closest open substitute if that ever needs solving.

## 2 · Brand & accent colors

One accent, Apple-style: `#7C3AED` (Violet 600) is THE Meridian accent — every
brand-interactive element (links, primary buttons, focus rings) uses it, the
same way Apple uses a single Action Blue. Formalized as `--t-accent` per theme in
`ui/app/globals.css` (light themes: `#7C3AED`; the `ink` dark theme uses the
lighter `#DCCFFB` tint for contrast, matching the existing `--gen-title` pattern).
The legacy amber/gold accent (`#C4822A` light / `#D4942E` dark) that drove the
setup wizard and a few older dialogs has been retired in favor of this same
violet — one accent, app-wide, no second brand identity left over from the
pre-Timeline design.

Functional palettes stay separate from this brand accent, since they carry
meaning a single accent color can't: the semantic status colors (§3) and the
10 task-category colors (`.cat-*` in `globals.css`) are untouched by this
simplification.

### Gradient — one flourish only

Apple's whole system has zero decorative gradients, reserving its one visual
flourish (a product-photo drop-shadow) for a single spot. Meridian's equivalent:
exactly one gradient survives, `--mer-pill-bg` (`ui/app/globals.css`) — the dark
violet lockup behind the "Meridian" wordmark in the toolbar nav pill, the one
fixed, always-visible brand mark. Every other surface that used to carry a
gradient (the primary button, the draft/review card header, tray's review-cta)
is now a flat `var(--t-accent)` fill:

| Element | Treatment | Token |
|---|---|---|
| Toolbar nav pill (the one flourish) | `linear-gradient(135deg,#332A63,#241C49)` | `--mer-pill-bg` |
| Primary button | Flat `var(--t-accent)` | `--btn-primary-bg` (`ui/app/globals.css`) |
| Draft / review card header | Flat `var(--t-accent)` | `--draft-card-bg` (`ui/app/globals.css`) |
| Approve button | `linear-gradient(135deg, #059669, #10B981)` (unchanged — semantic "success," not brand) | — |

Structural surface gradients — the per-theme window background (`--win-bg`),
desk backdrop (`--desk`), ambient `--glow`, and the theme-swatch `--chip` preview
— are unaffected; those are the window/background treatment, not brand
decoration, and remain as documented in §4.

## 3 · Semantic status colors

| State | Foreground | Background | Accent bar |
|---|---|---|---|
| Approved | `#0F9D6E` | `#E9F9F1` | `#10B981` |
| Needs review (log) | `#B4690E` | `#FEF5E7` | `#F59E0B` |
| New ticket (proposal) | `#7C3AED` | `#F4ECFE` | `#8B5CF6` |
| Dismissed | `#9C98AC` | `#F1F2F6` | `#C7C2D6` |
| Capturing pill | `#0F9D6E` | `#EDFAF2` | border `#CFEEDD` |
| Reject / dismiss button | `#D6486A` | `#FEF3F5` | border `#F3D4D9` |

## 4 · Surface tokens — the three themes

Every surface swaps by theme. Lilac (default, cool violet-white), Lavender (deeper all-violet — implemented as `blush`), Ink (dark). Each window background is a 4-stop diagonal gradient.

| Token | Lilac | Lavender | Ink |
|---|---|---|---|
| titleC | `#211D3D` | `#241E3D` | `#F4F1FC` |
| mutedC | `#6E6A88` | `#6F6890` | `#BBB2DC` |
| faintC | `#948FB8` | `#9791BC` | `#9A8FC2` |
| faint2C | `#ACA6CE` | `#AEA7CE` | `#84789F` |
| panelBg | `#FAF8FF` | `#FAF8FF` | `#211C48` |
| toolbarBg | grad `F9F7FE→F1ECFB` | grad `F7F3FE→EDE4FB` | grad `241F52→1B1740` |
| hairC (hairline) | `#E4DDF7` | `#DFD2F5` | `rgba(255,255,255,.09)` |
| cardBg | `#FFFFFF` | `#FFFFFF` | `#2E2864` |
| cardBorder | `#E9E3F8` | `#E5DCF8` | `rgba(255,255,255,.1)` |
| ctrlBg / ctrlBorder | `#FFF` / `#E4DEF6` | `#FFF` / `#E1D6F5` | `rgba(w,.08)` / `(w,.14)` |
| wrapBg (segmented) | `#EFEAFB` | `#EDE5FC` | `rgba(255,255,255,.07)` |
| rowHoverBg | `#F1ECFE` | `#EFE8FD` | `rgba(255,255,255,.06)` |
| trackBg (bar track) | `#EEE9FB` | `#ECE3FC` | `rgba(255,255,255,.1)` |
| boxBg (activity box) | `#F4F0FD` | `#F2ECFD` | `rgba(255,255,255,.055)` |
| inputBg / inputBorder | `#FDFCFF` / `#E1D7FA` | `#FCFBFF` / `#DED1F9` | `rgba(w,.07)` / `(w,.18)` |
| keyBg / keyText | `#EEE9FB` / `#3D3860` | `#ECE3FC` / `#3D3560` | `rgba(w,.1)` / `#E9E4F8` |

### Window background gradients (165°)

| Theme | Stops |
|---|---|
| Lilac | `#FCFBFF 0% → #EEE9FC 48% → #DCD3F3 78% → #CFC2ED 100%` |
| Lavender | `#F9F6FF 0% → #E7DEFA 46% → #CFBEF1 78% → #B8A2E8 100%` |
| Ink | `#332B72 0% → #211C4A 45% → #171331 78% → #0E0C1F 100%` |

### Desk backdrop (behind window) + blur glow

| Theme | Value |
|---|---|
| Lilac desk | `radial-gradient(1100px 760px at 18% −12%, #ECEAFF, #E7ECFC 45%, #E9EAF5)` |
| Lavender desk | `radial-gradient(1100px 760px at 82% −10%, #EDE7FC, #E3DEFA 46%, #E6E3F6)` |
| Ink desk | `radial-gradient(1000px 720px at 20% −8%, #2E2658, #201A3E 46%, #14112A)` |
| Glow | 1000×700 radial, blur 90px — violet/pink per theme |

## 5 · Spacing, radii, borders

| Property | Scale | Notes |
|---|---|---|
| Radius — window | 20px | Outer app frame |
| Radius — modals / big cards | 18–22px | Review card 22, cleanup/plan 20, draft card 18 |
| Radius — cards / boxes | 14–15px | Worklog, activity box, insight tiles |
| Radius — inputs / small | 10–12px | Textareas, task rows, pills |
| Radius — chips / buttons | 8–11px | Segmented 8, control btns 9 |
| Radius — status pill / dots | 999px | Fully round |
| Panel padding | 22px | Right-panel inner |
| Card padding | 13–16px | Timeline 13×15, detail 15×16 |
| Gap — card grid / groups | 7–11px | Task list 7, worklogs 11 |
| Hairline border | 1px | hairC per theme |
| Accent bar (card left) | inset 3px | Via inset box-shadow, status-colored |

## 6 · Shadows & elevation

Apple's rule — elevation only for the one floating/hero element, never for
ordinary resting cards — is the model here too. Meridian's shadow language keeps
elevation for what's actually floating (the app window, modals, the one
single-focus swipe/review card visible at a time, the floating drafts pill) and
relies on surface-color/border contrast for everything else at rest. An
always-on decorative shadow was removed from nothing that turned out to be a
true resting list card in this pass — investigation found Meridian's existing
shadow usage was already close to this discipline (see `docs/` history / Phase 2
audit); the two candidates flagged for removal (`CleanupCard`, `ReviewCard`)
turned out to be the single-focus swipe card below, not resting list items, so
their shadow stays.

| Element | Shadow |
|---|---|
| Window (light) | `inset 0 0 0 1px rgba(30,20,70,.05), 0 2px 6px …, 0 34px 70px −24px rgba(40,25,90,.42), 0 70px 120px −50px rgba(70,40,130,.32)` |
| Window (Ink) | `inset 0 0 0 1px rgba(255,255,255,.08), 0 34px 70px −24px rgba(0,0,0,.65), 0 70px 120px −50px rgba(110,60,200,.4)` |
| Worklog card | `inset 3px 0 0 [accent], 0 1px 2px …, 0 8px 20px −12px rgba(40,30,90,.22)` |
| Card hover | `…0 16px 32px −12px rgba(40,30,90,.32) + translateY(−2px)` |
| Draft card | `0 24px 60px −18px rgba(20,16,40,.5)` (flat `--draft-card-bg` fill now, not a gradient — see §2) |
| Swipe card (single-focus, one at a time) | `0 30px 60px −20px rgba(0,0,0,.5)` |
| Modal overlay | `bg rgba(20,16,40,.5) + backdrop-filter blur(3px)` |
| Floating drafts pill | `0 18px 40px −12px rgba(0,0,0,.6) on #15132A` |

## 7 · Layout & key components

| Region | Metrics |
|---|---|
| Window | 1280 × 840, flex column |
| Traffic-light chrome | height 46px · dots 12px (`#FF5F57` / `#FEBC2E` / `#28C840`) — **N/A, dropped**: Tauri provides real window chrome |
| Toolbar | height 60px · pad 0 22px · gap 16px |
| Right panel | width 388px, fixed · 1px left hairline |
| Timeline row | grid 62px + 1fr · min-height 54px · top hairline |
| Two tickets in an hour | flex row, gap 10px, side-by-side (equal 1fr) |
| Now-line | 2px `#EC4899` · 10px dot + nowPing ripple |
| Segmented toggle | pad 3px · active `#fff` + shadow `0 1px 3px` |
| Theme swatches | 22px, radius 7 · active ring `rgba(139,92,246,.25)` |
| Avatar | 34px round · indigo→pink gradient |
| Insight tiles | 3-col grid · gap 8 · gradient fills (violet / pink / cyan) |
| App-time bars | 8px track · per-app brand color · min label 42px mono |
| Toggle switch | 34×20 track · 16px knob · on `#8B5CF6` |
| Swipe threshold | ±120px → commit · rotate dx×0.04 · fly 760px / .24s |
| Swipe action buttons | 58px reject/approve, 48px edit · round |

## 8 · Motion & scrollbar

| Motion | Spec |
|---|---|
| nowPing keyframe | scale 1→2.6, opacity .55→0 · 2s ease-out infinite |
| Swipe drag | transform .04s linear (live) · commit .24s ease |
| Row / toggle transitions | background .15s · toggle .2s |
| Card hover | box-shadow + transform .2s |
| Scrollbar | 9px · thumb `rgba(90,90,140,.18)` → `.32` hover · radius 99px |

### App-brand colors for time bars

Single source of truth: `ui/lib/brand-icons.ts`'s `BRAND_ICONS` map (real hex + vector wordmark per app, sourced from simple-icons/Font Awesome Free — see that file's header for provenance and how to add/refresh an entry). `TimeByApp.tsx`'s `appColor()` reads `BRAND_ICONS[app]?.hex`; any app not in the map falls back to a deterministic hashed hue (`appHue()`), not a real brand color. Do not hand-maintain a second copy of this list here — it drifts (this table used to list VS Code/Jira/GitHub/Postman/Gmail/DevTools, none of which are actually in the map).

Currently mapped: Google Chrome `#4285F4` · Claude `#D97757` · Claude Code `#D97757` · Arc `#FCBFBD` · WhatsApp `#25D366` · DBeaver `#382923` · Zoom `#0B5CFF` · Linear `#5E6AD2` · Figma `#F24E1E` · Notion `#000000` · Spotify `#1ED760` · Safari `#006CFF` · Xcode `#147EFB` · iTerm2 `#000000` · Slack `#4A154B`. ChatGPT has no entry — no redistributable OpenAI wordmark exists in either icon source — so it keeps the letter-monogram fallback like Apple's own system apps.

## 9 · Design standard — rules for new work (prescriptive)

Sections 1–8 document what the mock defines — a catalog of what exists. This section is normative going forward: it governs what a *new* banner, badge, card, or accent may use. Added 2026-07-06, driven by the standardized-action-cards effort, which surfaced near-duplicate status colors (two greens, three ambers, two reds) and inconsistent banner styling across `MustFixBanner`, the cleanup card, and the drafts card.

### 9.1 · Color budget — 3 accent colors, no more

Every semantic/status/UI-accent use must map onto exactly these three. No new hue may be introduced without retiring one of the three first.

| Color | Token | Value | Meaning |
|---|---|---|---|
| Violet | `--color-state-proposal` | `#8B5CF6` | Brand identity · informational · AI-authored/proposed content · drafts |
| Green | `--color-state-approved` | `#10B981` | Positive · success · approved · done |
| Amber | `--color-state-pending` | `#F59E0B` | Needs attention — pending, warning, **and** urgent/must-fix. Severity is conveyed by icon + copy + priority order, never by a 4th color. |

These are already the three most-used accent tokens in `ui/` (81 / 43 / 35 references respectively) — this budget consolidates onto the existing winners rather than inventing a new palette.

**Not counted against the budget** (structural, not accent): the neutral grayscale — `--t-title` / `--t-muted` / `--t-faint` / `--t-faint-2` / `--t-hair` / `--t-card-border` — and the "dismissed/rejected" gray `--color-state-rejected` (`#C7C2D6`). Every UI needs a text/border hierarchy independent of how many accent colors it has.

**Retired — fold into the 3 above, do not use in new work:**

| Retired token(s) | Value | Folds into |
|---|---|---|
| `--accent` / `--success` / `--warn` (legacy palette — "kept for Sessions/Week/setup") | `#C4822A` / `#2D7A4F` / `#A36A1A` | Violet / Green / Amber |
| `--severity-must` | `#EF4444` | Amber — urgency comes from icon + copy + priority order |
| `--severity-nice` | `#F97316` | Amber |
| `--status-info-*` | `#eff6ff`/`#bfdbfe`/`#1d4ed8`/`#2563eb` | Violet |
| `--status-warning-*` | `#fffbeb`/`#fcd34d`/`#92400e`/`#d97706` | Amber |
| `--status-error-*` | `#fff5f5`/`#feb2b2`/`#c53030`/`#e53e3e` | Amber |

Precedent already shipped: the action-card stack (`ActionCard.tsx` / `useActionItems.ts`) uses one uniform neutral card style with zero color-tiering — must-fix, cleanup, and drafts differ only by icon, copy, and stack order, never by color.

### 9.2 · Gradients — static is brand, animated is "AI is working"

Static gradients are the app's brand chrome and are **not** feature-specific — the logo, window background, primary buttons, and theme swatches keep their signature violet→pink diagonal (§2, §4) regardless of what they're attached to. Don't remove them and don't gate new static gradients behind "is this an AI feature."

The **animated/shimmering** gradient treatment (background-position animation — `mer-gen-shimmer`) is reserved exclusively for "a model is actively generating output right now." Today that's the `--gen-*` hour-takeover card during the live `/worklog_hour` call (§7, `HourBadges.tsx`). Do not add a shimmering gradient to a static element, and do not add a new animated gradient for anything that isn't a live model call in flight.

| Gradient type | Where | Rule |
|---|---|---|
| Static (chrome) | Logo/avatar, window bg, primary buttons, theme swatches, ReportModal hero | Always allowed — brand identity, unrelated to feature |
| Animated (shimmer) | `--gen-*` worklog-generating takeover card | Reserved for "LLM call in flight" — never reused for decoration |

### 9.3 · Categorical data-viz colors — resolved 2026-07-09

The 3-color budget (§9.1) governs semantic/status/UI-accent colors. Three existing surfaces use wider categorical palettes — the activity category chart legend (`.cat-*` in `globals.css`, 10 hues), the app-brand colors for time bars (§8 above, 13 colors, each anchored to a tracked app's real logo color), and the epic hash palette (`EPIC_PALETTE` in `TasksPanel.tsx`, 8 hues). **Decision: no exception for any of the three — all categorical color, including app-brand, is remapped onto the same 3 hue families as §9.1.** This was chosen over carving out a brand-color exception, with the recognizability cost below accepted knowingly rather than deferred.

**The 3 families, as a tint/shade scale** (not single flat colors — each family spans a lightness range so a legend can carry multiple items per family):

| Family | Hue / saturation anchor | Range |
|---|---|---|
| Violet | H258° S88% (anchored to `--color-state-proposal` `#8B5CF6`) | L28%→L82% |
| Green | H160° S84% (anchored to `--color-state-approved` `#10B981`) | L28%→L82% |
| Amber | H38° S92% (anchored to `--color-state-pending` `#F59E0B`) | L28%→L82% |

**Assignment rule:** for each categorical surface independently (the category legend, the brand time-bars, the epic palette are three separate contexts — they don't need to be mutually distinct from each other, only within their own legend), map every existing color to its nearest hue family by hue distance, then spread the items landing in the same family across that family's lightness range (darkest original → darkest new, preserving relative rank) so they stay separable within one legend.

**Known cost, measured, not hand-waved:** hue-distance mapping is not evenly distributed across the 3 families, because the source palettes lean warm (reds/oranges/pinks/browns cluster to amber; blues cluster to violet). Concretely: the category legend maps 5/10 items into the amber family and only 2/10 into green; the brand time-bars map 8/13 into violet and only 2/13 into green (chrome, slack, figma, gmail, postman all land in amber alongside meeting/deployment_devops/design/planning/idle_personal — five same-family shades in one 10-item legend is a real legibility cost, and Slack/Jira/Chrome/VS Code all losing their real brand hue is a real recognizability cost). This is the tradeoff §9 originally flagged as "a different kind of harm" — recorded here as accepted, not silently smoothed over. If this reads worse than expected once it's on screen, the fallback is reopening this section for a brand-color exception, not inventing a workaround inline at the call site.

**Deferred to implementation** (not this doc): the exact per-color hex assignment. A blind desk mapping computed without seeing it rendered risks locking in a bad legend (e.g. two amber shades that read as near-identical at chart scale) — the concrete hex-to-token mapping for all 31 colors (10 category + 13 brand + 8 epic) is the first task of the implementation pass, done with the running chart/legend in front of you, not authored into this standards doc sight-unseen.
