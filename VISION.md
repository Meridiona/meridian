# Vision

> "Data is the new oil — but only if you can refine it." 

## What Meridian is

The structured activity layer between raw screen recordings and AI reasoning.

Screenpipe captures everything — OCR frames, audio transcriptions, accessibility tree events — at high fidelity, 24/7. That data is rich but raw: a flat stream of frames with no concept of what you were doing, when a task started, or how long you focused. Meridian is the refinery. It reads that stream, detects app-switch boundaries, extracts context from each block, and writes clean session records that AI can actually reason over.

Not an analytics tool. Not a time tracker you fill out. An automated normalization layer that runs in the background and asks nothing of you.

## Why we exist

Raw frames are not queryable intent. An AI agent looking at screenpipe's frame table sees noise — thousands of rows with no session structure, no duration, no concept of "I worked on this for 40 minutes." Before any AI can answer "what did I work on today?", someone has to do the ETL.

Meridian does that ETL. Zero config, zero prompts, zero UI interaction required. It runs alongside screenpipe, consumes minimal resources, and produces a clean SQLite database that any tool — query, dashboard, AI agent — can read immediately.

## Where this goes

1. **Now: Reliable ETL.** Convert screenpipe's frame stream into correct, complete, deduplicated app sessions. No phantom sessions, no gaps, no duration errors. The foundation has to be right before anything else matters.

2. **Next: AI context via MCP.** The MCP server makes meridian's data available to any AI agent. Ask Claude what you worked on, how long you focused, which apps dominated your week — and get accurate answers sourced from your actual screen time.

3. **Later: Productivity intelligence.** Project-level session grouping, focus quality scoring, anomaly detection, cross-day patterns. The structured session data is the substrate for everything.

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
