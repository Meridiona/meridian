#!/usr/bin/env node
// meridian — AI activity intelligence by Meridiona

import * as fs from "fs";
import * as path from "path";
import * as os from "os";

type McpEntry = {
  type?: string;
  command: string;
  args: string[];
  env?: Record<string, string>;
};

type McpConfig = {
  mcpServers?: Record<string, McpEntry>;
  [key: string]: unknown;
};

// ─── Config paths ─────────────────────────────────────────────────────────────

function getClaudeDesktopConfigPath(): string {
  const home = os.homedir();
  if (process.platform === "win32") {
    const appData = process.env.APPDATA ?? path.join(home, "AppData", "Roaming");
    return path.join(appData, "Claude", "claude_desktop_config.json");
  }
  if (process.platform === "linux") {
    const xdg = process.env.XDG_CONFIG_HOME ?? path.join(home, ".config");
    return path.join(xdg, "Claude", "claude_desktop_config.json");
  }
  return path.join(home, "Library", "Application Support", "Claude", "claude_desktop_config.json");
}

// ~/.claude.json — Claude Code (CLI) user-level MCPs
function getClaudeCodeUserConfigPath(): string {
  return path.join(os.homedir(), ".claude.json");
}

// .mcp.json in cwd — Claude Code project-level MCPs
function getProjectMcpConfigPath(): string {
  return path.join(process.cwd(), ".mcp.json");
}

function getCursorConfigPath(): string {
  return path.join(os.homedir(), ".cursor", "mcp.json");
}

function getWindsurfConfigPath(): string {
  return path.join(os.homedir(), ".codeium", "windsurf", "mcp_config.json");
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

function readJson(filePath: string): Record<string, unknown> {
  try {
    if (fs.existsSync(filePath)) {
      return JSON.parse(fs.readFileSync(filePath, "utf-8")) as Record<string, unknown>;
    }
  } catch { /* treat as empty on parse error */ }
  return {};
}

function writeJson(filePath: string, data: Record<string, unknown>): void {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, JSON.stringify(data, null, 2) + "\n", "utf-8");
}

// Standard entry for Claude Desktop, Cursor, Windsurf, .mcp.json
function buildEntry(): McpEntry {
  const entry: McpEntry = { command: "npx", args: ["-y", "meridian-mcp@latest"] };
  const dbPath = process.env.MERIDIAN_DB;
  if (dbPath) entry.env = { MERIDIAN_DB: dbPath };
  return entry;
}

// Claude Code user-level entry — needs "type": "stdio" and always has env key
function buildClaudeCodeEntry(): McpEntry {
  const entry: McpEntry = {
    type: "stdio",
    command: "npx",
    args: ["-y", "meridian-mcp@latest"],
    env: {},
  };
  const dbPath = process.env.MERIDIAN_DB;
  if (dbPath) entry.env = { MERIDIAN_DB: dbPath };
  return entry;
}

function installMcpServers(configPath: string, entry: McpEntry): { updated: boolean } {
  const config = readJson(configPath);
  if (!config.mcpServers || typeof config.mcpServers !== "object") config.mcpServers = {};
  const servers = config.mcpServers as Record<string, McpEntry>;
  const updated = !!servers["meridian"];
  servers["meridian"] = entry;
  writeJson(configPath, config);
  return { updated };
}

// For ~/.claude.json the mcpServers lives at the TOP LEVEL (not nested in a project)
function installClaudeCodeUser(configPath: string): { updated: boolean } {
  return installMcpServers(configPath, buildClaudeCodeEntry());
}

// ─── Installer ────────────────────────────────────────────────────────────────

export function runInstaller(): void {
  console.log("\nMeridian MCP Installer\n");

  let any = false;

  function tryInstall(
    name: string,
    configPath: string,
    installer: (p: string) => { updated: boolean },
    requireDirExists = false,
  ): void {
    if (requireDirExists && !fs.existsSync(path.dirname(configPath))) return;
    try {
      const { updated } = installer(configPath);
      any = true;
      const verb = updated ? "↻ Updated " : "✓ Installed";
      console.log(`  ${verb} ${name}`);
      console.log(`    ${configPath}`);
    } catch (err) {
      console.log(`  ✗ ${name} — ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  // Claude Code — user level (~/.claude.json)
  tryInstall("Claude Code (user)", getClaudeCodeUserConfigPath(), installClaudeCodeUser, false);

  // Claude Code — project level (.mcp.json in cwd)
  tryInstall(
    "Claude Code (project)",
    getProjectMcpConfigPath(),
    (p) => installMcpServers(p, buildEntry()),
    false,
  );

  // Claude Desktop
  tryInstall("Claude Desktop", getClaudeDesktopConfigPath(), (p) => installMcpServers(p, buildEntry()), false);

  // Cursor — only if ~/.cursor exists
  tryInstall("Cursor", getCursorConfigPath(), (p) => installMcpServers(p, buildEntry()), true);

  // Windsurf — only if ~/.codeium/windsurf exists
  tryInstall("Windsurf", getWindsurfConfigPath(), (p) => installMcpServers(p, buildEntry()), true);

  console.log();

  if (any) {
    console.log("Done. Restart any open clients to activate the server.");
    console.log();
    console.log("Available tools:");
    console.log("  get-sessions       — list app sessions by date");
    console.log("  get-timeline       — chronological day view with gaps");
    console.log("  get-stats          — focus/idle time and top apps");
    console.log("  get-active-session — currently active app");
    console.log("  get-apps           — all-time app usage");
    console.log("  search-sessions    — search OCR text, audio, window titles");
    console.log("  get-session-detail — full content for a session");
    console.log("  health-check       — ETL status and DB info");
  } else {
    console.log("No clients configured. Add manually to ~/.claude.json → mcpServers:");
    console.log();
    console.log(JSON.stringify({ meridian: buildClaudeCodeEntry() }, null, 2));
  }

  console.log();
}

// Run directly when this file is the entry point (npx meridian-mcp-install)
const scriptPath = process.argv[1] ?? "";
if (scriptPath.endsWith("install.js") || scriptPath.endsWith("install.ts")) {
  runInstaller();
}
