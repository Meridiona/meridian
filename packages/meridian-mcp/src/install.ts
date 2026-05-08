#!/usr/bin/env node
// meridian — AI activity intelligence by Meridiona

import * as fs from "fs";
import * as path from "path";
import * as os from "os";

type McpEntry = {
  command: string;
  args: string[];
  env?: Record<string, string>;
};

type McpConfig = {
  mcpServers?: Record<string, McpEntry>;
  [key: string]: unknown;
};

// ─── Config paths ─────────────────────────────────────────────────────────────

function getClaudeConfigPath(): string {
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

function getCursorConfigPath(): string {
  return path.join(os.homedir(), ".cursor", "mcp.json");
}

function getWindsurfConfigPath(): string {
  return path.join(os.homedir(), ".codeium", "windsurf", "mcp_config.json");
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

function readConfig(filePath: string): McpConfig {
  try {
    if (fs.existsSync(filePath)) {
      return JSON.parse(fs.readFileSync(filePath, "utf-8")) as McpConfig;
    }
  } catch { /* ignore parse errors — treat as empty */ }
  return {};
}

function writeConfig(filePath: string, config: McpConfig): void {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, JSON.stringify(config, null, 2) + "\n", "utf-8");
}

function buildEntry(): McpEntry {
  const entry: McpEntry = { command: "npx", args: ["-y", "meridian-mcp@latest"] };
  const dbPath = process.env.MERIDIAN_DB;
  if (dbPath) entry.env = { MERIDIAN_DB: dbPath };
  return entry;
}

function installInto(configPath: string): { updated: boolean } {
  const config = readConfig(configPath);
  if (!config.mcpServers || typeof config.mcpServers !== "object") config.mcpServers = {};
  const updated = !!config.mcpServers["meridian"];
  config.mcpServers["meridian"] = buildEntry();
  writeConfig(configPath, config);
  return { updated };
}

// ─── Installer ────────────────────────────────────────────────────────────────

export function runInstaller(): void {
  console.log("\nMeridian MCP Installer\n");

  const targets: Array<{ name: string; path: string }> = [
    { name: "Claude Desktop", path: getClaudeConfigPath() },
    { name: "Cursor", path: getCursorConfigPath() },
    { name: "Windsurf", path: getWindsurfConfigPath() },
  ];

  let any = false;

  for (const { name, path: configPath } of targets) {
    // Only install into Cursor/Windsurf if their config dir already exists
    const dir = path.dirname(configPath);
    if (name !== "Claude Desktop" && !fs.existsSync(dir)) continue;

    try {
      const { updated } = installInto(configPath);
      any = true;
      console.log(`  ${updated ? "↻ Updated" : "✓ Installed"} ${name}`);
      console.log(`    ${configPath}`);
    } catch (err) {
      console.log(`  ✗ ${name} — ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  console.log();

  if (any) {
    console.log("Restart Claude Desktop / Cursor / Windsurf to activate the MCP server.");
    console.log();
    console.log("Available tools:");
    console.log("  get-sessions      — list app sessions by date");
    console.log("  get-timeline      — chronological day view with gaps");
    console.log("  get-stats         — focus/idle time and top apps");
    console.log("  get-active-session— currently active app");
    console.log("  get-apps          — all-time app usage");
    console.log("  search-sessions   — search OCR text, audio, window titles");
    console.log("  get-session-detail— full content for a session");
    console.log("  health-check      — ETL status and DB info");
  } else {
    console.log("No supported clients found. Add this manually to claude_desktop_config.json:");
    console.log();
    console.log(JSON.stringify({ mcpServers: { meridian: buildEntry() } }, null, 2));
  }

  console.log();
}

// Run directly when this file is the entry point (npx meridian-mcp-install)
const scriptPath = process.argv[1] ?? "";
if (scriptPath.endsWith("install.js") || scriptPath.endsWith("install.ts")) {
  runInstaller();
}
