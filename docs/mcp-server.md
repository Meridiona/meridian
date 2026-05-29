# MCP Server

Meridian ships a TypeScript MCP server that exposes your session data to any MCP-compatible AI tool — Claude Code, Claude Desktop, Cursor, and others.

## Setup

`./install.sh` builds the MCP server automatically. To rebuild manually:

```bash
cd packages/meridian-mcp
npm run build
```

### Add to Claude Desktop

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

### Add to Claude Code

```bash
claude mcp add meridian node /path/to/meridian/packages/meridian-mcp/dist/index.js
```

## Available tools

| Tool | Description |
|---|---|
| `get-sessions` | Fetch sessions for a date range |
| `get-timeline` | Day-level timeline of sessions |
| `get-stats` | Aggregated stats (total time, top apps) |
| `get-active-session` | Currently open session |
| `get-apps` | Per-app breakdown |
| `search-sessions` | Full-text search across OCR, window titles, audio |
| `get-session-detail` | Full detail for a single session by ID |
| `health-check` | Verify the server can reach `meridian.db` |

## Notes

- **Audio excluded from responses** — `audio_snippets` are stored in the DB but intentionally excluded from MCP tool responses to reduce LLM noise. They remain searchable via `search-sessions`.
- **Read-only** — the MCP server opens `meridian.db` with `readonly: true`. No accidental writes.
- **Custom DB path** — set `MERIDIAN_DB=/path/to/meridian.db` in the environment before starting the server.
- **WebAssembly SQLite** — uses `sql.js` (pure WASM). No native Node.js modules; works with any Node.js version.

## Startup behaviour

If `~/.meridian/meridian.db` does not exist when the server starts, it returns a clear error on the first tool call rather than crashing. Start the Rust daemon at least once to create the database.
