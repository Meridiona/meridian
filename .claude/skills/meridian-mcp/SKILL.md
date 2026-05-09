---
name: meridian-mcp
description: "Build, configure, and debug the Meridian MCP server. Exposes app session data to AI tools via the Model Context Protocol."
allowed-tools: Bash, Read, Edit, Grep, Write
---

# Meridian MCP Server Skill

## What It Is

A TypeScript MCP server (`packages/meridian-mcp/`) that exposes Meridian's structured session database to any MCP-compatible AI tool (Claude Code, Claude Desktop, Cursor, etc.).

It opens `~/.meridian/meridian.db` read-only using **sql.js** (pure WebAssembly SQLite — no native Node.js modules, works with any Node.js version) and provides tools for querying app sessions, focus time, and activity history.

## Build & Run

```bash
cd packages/meridian-mcp

# Install dependencies
npm install

# Build TypeScript → dist/
npm run build

# Start server (stdio transport)
node dist/index.js
```

**Prerequisite**: The Meridian daemon must be running and have produced at least one session. The MCP server returns an error message on tool calls (does not crash) if the DB is missing.

## Available Tools

| Tool | Description |
|------|-------------|
| `get-sessions` | List completed app sessions for a date (default: today) |
| `get-timeline` | Full day timeline including idle and sleep gaps |
| `get-stats` | Daily productivity stats — focus/idle/away time, top apps |
| `get-active-session` | Currently in-progress session (if daemon is running) |
| `get-apps` | All-time app usage stats |
| `search-sessions` | Search sessions by window title, OCR text, or audio |
| `get-session-detail` | Full OCR, elements, and signals for a session ID (audio excluded — stored in DB but not sent to LLMs) |
| `health-check` | ETL run status, cursor position, and total session count |

## Configuration

```bash
# Override DB path (default: ~/.meridian/meridian.db)
MERIDIAN_DB=/path/to/custom.db node dist/index.js
```

## Add to Claude Desktop / Claude Code

Edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "meridian": {
      "command": "node",
      "args": ["/path/to/meridian/packages/meridian-mcp/dist/index.js"]
    }
  }
}
```

Replace `/path/to/meridian` with the actual repo path.

## Debugging

### MCP server errors on startup
```bash
# Check if DB exists
ls -lh ~/.meridian/meridian.db

# Check if daemon is running
pgrep meridian

# Run daemon first, then start MCP server
./target/release/meridian &
node packages/meridian-mcp/dist/index.js
```

### TypeScript build errors
```bash
cd packages/meridian-mcp
npx tsc --noEmit    # type-check without emitting
npm run build       # full build
```

### Test a tool call manually
```bash
npx @modelcontextprotocol/inspector node dist/index.js
```

## Development

```bash
# Watch mode (ts-node)
npm run dev

# Type check only
npx tsc --noEmit

# Add a new tool: edit src/index.ts
# 1. Add to the TOOLS array (name, description, inputSchema)
# 2. Add a case in the CallToolRequest handler
# 3. Rebuild: npm run build
```

## File Structure

```
packages/meridian-mcp/
├── src/
│   └── index.ts          # All MCP logic (single file)
├── dist/                 # Compiled JS (committed)
├── package.json
└── tsconfig.json
```

## Implementation Notes

- Uses `sql.js` (pure WASM) instead of `better-sqlite3` (native) to avoid Node.js version mismatch errors across different environments (Claude Code, Claude Desktop, shell).
- The DB file is loaded into memory on each tool call so data is always fresh (daemon writes every 60s).
- The sql.js engine itself (`_SQL`) is cached across calls to avoid re-initialising the WASM runtime.
- `audio_snippets` is intentionally excluded from all MCP tool responses. Audio transcriptions from screenpipe are noisy and prone to hallucinations. They are still stored in `app_sessions.audio_snippets` and remain searchable via `search-sessions` (LIKE match on the column).
