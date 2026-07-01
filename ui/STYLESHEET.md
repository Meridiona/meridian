//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

# Meridian Timeline — Style Sheet

The complete visual system for the Meridian daily-timeline desktop app: type, color tokens across all three themes, spacing, radii, shadows, and every component's exact values. All styling is applied inline in the source mock (no CSS classes); in our implementation the equivalent values live in `ui/app/globals.css` (`.mt-*` type classes + `--t-*`/`--color-*` tokens). This document is the source of truth for what those must match. Fixed canvas in the mock: 1280 × 840px (our shell is fluid — see `MeridianTimelineShell.tsx`).

## 1 · Typography

Two families. Plus Jakarta Sans for all UI text; JetBrains Mono for ticket keys, times, durations and numeric labels. Loaded from Google Fonts.

Plus Jakarta Sans — 400 / 500 / 600 / 700 / 800
JetBrains Mono — 400 / 500 / 600 / 700

| Role | Family | Size | Weight | Line / Tracking |
|---|---|---|---|---|
| Panel heading ("You had a solid day") | Jakarta | 20px | 800 | 1.25 / −.02em |
| Modal title (Review, Cleanup, Plan) | Jakarta | 17–22px | 800 | 1.2 / −.02em |
| Swipe-card ticket title | Jakarta | 19px | 800 | 1.3 / −.02em |
| Toolbar date | Jakarta | 15px | 700 | 1.1 |
| Worklog card title | Jakarta | 13.5px | 700 | 1.3 / −.01em |
| Body / summary text | Jakarta | 12–14px | 500 | 1.45–1.55 |
| Section label (TODAY'S FOCUS) | Jakarta | 11px | 700 | / .08em, UPPER |
| Ticket key (MER-482) | Mono | 11–12px | 600–700 | / .02em |
| Hour label / time / duration | Mono | 10.5–11px | 600 | / .02em |
| Status pill (NEEDS REVIEW) | Mono | 9.5px | 700 | / .03em, UPPER |

Global smoothing: `-webkit-font-smoothing: antialiased`. Base reset: `*{box-sizing:border-box}`, `html,body{margin:0;padding:0}`.

## 2 · Brand & accent colors

Theme-independent. Gradients are the signature — the violet→pink diagonal appears on the logo, avatar, primary buttons and draft card.

| Token | Value | Usage |
|---|---|---|
| Violet 700 | `#6D28D9` | Primary text-accent, focus numbers |
| Violet 600 | `#7C3AED` | Links, "Edit plan", proposal accent |
| Violet 500 | `#8B5CF6` | Accents, toggles, now-dot rings |
| Purple 500 | `#A855F7` | Gradient mid-stop |
| Pink 500 | `#EC4899` | Now-line, gradient end, meetings stat |
| Pink 600 | `#DB2777` | Meetings figure, gradient end (buttons) |
| Indigo 500 | `#6366F1` | Logo/avatar gradient start |
| Cyan 400 | `#22D3EE` | Tertiary (context-switch stat, chips) |

### Signature gradients

| Element | Gradient |
|---|---|
| Logo / avatar | `linear-gradient(135deg, #6366F1, #A855F7 55%, #EC4899)` |
| Primary button | `linear-gradient(135deg, #6D28D9, #9333EA)` |
| Draft / review card | `linear-gradient(135deg, #5B21B6, #7C3AED 45%, #DB2777)` |
| Approve button | `linear-gradient(135deg, #059669, #10B981)` |

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

| Element | Shadow |
|---|---|
| Window (light) | `inset 0 0 0 1px rgba(30,20,70,.05), 0 2px 6px …, 0 34px 70px −24px rgba(40,25,90,.42), 0 70px 120px −50px rgba(70,40,130,.32)` |
| Window (Ink) | `inset 0 0 0 1px rgba(255,255,255,.08), 0 34px 70px −24px rgba(0,0,0,.65), 0 70px 120px −50px rgba(110,60,200,.4)` |
| Worklog card | `inset 3px 0 0 [accent], 0 1px 2px …, 0 8px 20px −12px rgba(40,30,90,.22)` |
| Card hover | `…0 16px 32px −12px rgba(40,30,90,.32) + translateY(−2px)` |
| Draft card (gradient) | `0 16px 34px −12px rgba(124,58,237,.65), 0 4px 10px −6px …` |
| Swipe card | `0 30px 60px −20px rgba(0,0,0,.5)` |
| Modal overlay | `bg rgba(10,8,24,.5) + backdrop-filter blur(6px)` |
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

VS Code `#4F8FEF` · Chrome `#F4B400` · Safari `#2AA9FF` · Slack `#E01E5A` · Jira `#2684FF` · Figma `#F24E1E` · Zoom `#2D8CFF` · Notion `#3B3752` · GitHub `#57606A` · Postman `#FF6C37` · Gmail `#EA4335` · iTerm `#25A06A` · DevTools `#4285F4`.
