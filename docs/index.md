---
layout: home

hero:
  name: Meridian
  text: Ambient activity tracking for developers
  tagline: Turns screenpipe's raw frames into structured app sessions — automatically, locally, silently.
  actions:
    - theme: brand
      text: Get Started
      link: /getting-started/
    - theme: alt
      text: Architecture
      link: /architecture/

features:
  - title: Zero-effort tracking
    details: Runs alongside screenpipe. Detects app-switch boundaries, classifies sessions to Jira tasks, and posts updates — no UI interaction required.
  - title: Local-first, always
    details: No network calls, no telemetry, no remote dependencies. All data stays in a local SQLite database at ~/.meridian/meridian.db.
  - title: MCP-compatible
    details: Ships a TypeScript MCP server that exposes session data to Claude Code, Claude Desktop, Cursor, and any MCP-compatible AI tool.
  - title: PM sync
    details: Classifies each session to the specific ticket you're working on and posts timed progress comments to Jira automatically.
---
