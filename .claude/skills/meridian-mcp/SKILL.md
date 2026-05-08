---
name: meridian-mcp
description: "Build, configure, and debug the Meridian MCP server. Exposes app session data to AI tools via the Model Context Protocol."
allowed-tools: Bash, Read, Edit, Grep, Write
---

# Meridian MCP Server Skill

## What It Is

A TypeScript MCP server (`packages/meridian-mcp/`) that exposes Meridian's structured session database to any MCP-compatible AI tool (Claude Desktop, Cursor, etc.).

It opens `~/.meridian/meridian.db` read-only and provides tools for querying app sessions, focus time, and activity history.

## Build & Run

```bash
cd packages/meridian-mcp

# Install dependencies
npm install

# Build TypeScript → dist/
npm run build

# Start server (stdio transport)
npm start
# or
node dist/index.js
```

**Prerequisite**: The Meridian daemon must be running and have produced at least one session. The MCP server will error on tool calls (not crash) if the DB is missing.

## Available Tools

The server exposes these MCP tools (defined in `src/index.ts`):

| Tool | Description |
|------|-------------|
| `get_sessions` | List app sessions for a given date (defaults to today) |
| `get_focus_time` | Total time per app for a date range |
| `get_active_session` | Current in-progress session (if daemon is running) |
| `get_app_stats` | Usage stats aggregated by app over a time range |
| `search_sessions` | Find sessions containing specific OCR text or window title |

## Configuration

```bash
# Override DB path (default: ~/.meridian/meridian.db)
MERIDIAN_DB=/path/to/custom.db node dist/index.js
```

## Add to Claude Desktop

Edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "meridian": {
      "command": "node",
      "args": ["/path/to/meridian/packages/meridian-mcp/dist/index.js"],
      "env": {
        "MERIDIAN_DB": "/Users/yourname/.meridian/meridian.db"
      }
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
# Use the MCP inspector (if available)
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
├── dist/                 # Compiled JS (git-ignored)
├── package.json
└── tsconfig.json
```
