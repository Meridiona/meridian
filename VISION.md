# Vision

> "Data is the new oil — but only if you can refine it." 

## What Meridian is

The developer efficiency layer between raw screen activity and the tools developers already use.

Screenpipe captures everything — OCR frames, audio transcriptions, accessibility tree events — at high fidelity, 24/7, with no data leaving the machine. That data is rich but raw: a flat stream of frames with no concept of what you were doing, which ticket you were working on, or how long you focused. Meridian is the refinery and the bridge. It reads that stream, detects app-switch boundaries, classifies each session into the task it belongs to, and automatically updates the developer's project management tools — Jira, GitHub Issues, Linear — without them ever having to context-switch.

Not an analytics tool. Not a time tracker you fill out. An ambient automation layer that watches what you build and keeps your project management in sync, all locally, all silently.

## Why we exist

Developers lose hours every week on project management overhead: updating ticket status, logging time, writing standup notes, moving cards. The information needed to do all of this already exists on their screen — in the code they're writing, the PRs they're reviewing, the terminals they're running. Meridian captures that context, structures it, and pushes it to the right place automatically.

Zero config, zero prompts, zero UI interaction required. It runs alongside screenpipe, consumes minimal resources, and produces both a clean local SQLite database and live updates to external project management systems.

## Where this goes

1. **Done: Reliable ETL.** Correct, complete, deduplicated app sessions. Gap detection (user idle vs system sleep), OCR/audio/signal deduplication, single-frame duration fix, sleep gap boundary detection. The foundation is solid.

2. **Done: AI context via MCP.** The MCP server exposes structured session data to any MCP-compatible AI (Claude Code, Claude Desktop, Cursor). Sessions include window titles, OCR samples, accessibility elements, and signals. Audio is stored in the DB but excluded from LLM responses (noise reduction).

3. **Done: Activity categorization.** AI-assigned categories (`coding`, `meeting`, `research`, `communication`, `design`, `documentation`, `planning`, `deployment_devops`, `idle_personal`) with confidence scores. Category-aware UI: timeline coloring, session badges, daily breakdown chart.

4. **Now: Task classification and PM sync.** Classify each session into the specific task or ticket it belongs to — using OCR text, window titles, URLs, git branch names, and terminal context. Aggregate task-linked sessions and automatically update the corresponding ticket on Jira, GitHub Issues, Linear, or any connected PM tool. The developer never touches a ticket; the work updates it.

5. **Next: Cross-session task aggregation.** A single task spans many sessions across many hours or days. Build the aggregation layer: sum time, collect evidence (PRs opened, files changed, terminals run), and produce a rich activity log per task that feeds into status updates, standup summaries, and sprint reviews.

6. **Later: Productivity intelligence.** Focus quality scoring (coding vs meeting ratio, context-switch frequency), cross-day patterns, anomaly detection. The task-linked session data is the substrate for understanding how developers actually spend their time vs how they planned to.

## Product principles

- **Correctness over features.** A wrong session boundary is worse than no feature. ETL accuracy is non-negotiable.
- **Minimal footprint.** Meridian runs 24/7 alongside screenpipe. Target: <1% CPU idle, <50MB RAM. Never compete with the tools being recorded.
- **Local-first always.** Data stays on the machine. No network calls, no telemetry, no remote dependencies. Encryption at rest is the user's choice.
- **No feature creep.** Every feature must serve the ETL or make the session data more queryable. If it doesn't, it doesn't ship.

## Engineering principles

- **Idempotent ETL.** Re-running on the same data must produce the same result. No duplicates, ever.
- **Migrations must be safe.** Every schema change must be backward-compatible. Existing rows survive every migration.
- **Test the data path first.** Integration tests over the ETL runner are more valuable than unit tests over helpers.
- **Read-only on screenpipe.** Meridian never writes to screenpipe's DB. Always open it with read-only flags.
- **Fail loud, recover gracefully.** Startup errors (missing screenpipe DB, corrupt meridian DB) must surface clearly. Runtime errors (bad frame, parse failure) must be logged and skipped, not crashed.

## North star metrics

- **ETL accuracy** — sessions correctly bounded (no phantom sessions, no missed boundaries).
- **Data completeness** — zero gaps between cursor and latest screenpipe frame after each poll.
- **MCP query latency** — AI tool responses under 200ms for a full day's session data.

## What we believe

- Ambient data should require zero user effort to structure.
- AI tools are only as good as the context they receive. Bad data in, bad answers out.
- Local computation is a feature. Privacy through architecture, not promises.
- The boring infrastructure — reliable ETL, stable schemas, correct math — is what makes everything else possible.
