#!/usr/bin/env python3
"""Bootstrap — initialize state directories, state files, and configure Screenpipe MCP."""
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))
from agents.config import (
    HERMES_HOME,
    ACTIVITY_DIR, JIRA_DIR, MEMORIES_DIR,
    BUFFER_FILE, CONTEXT_MAP_FILE, CURRENT_CONTEXT_FILE,
    JIRA_STATE_FILE, TICKET_MAPPINGS_FILE,
)


def init_directories():
    print("Creating directories...")
    for d in [ACTIVITY_DIR, JIRA_DIR, MEMORIES_DIR, HERMES_HOME / "logs"]:
        d.mkdir(parents=True, exist_ok=True)
        print(f"  ✓ {d}")


def init_state_files():
    print("\nInitializing state files...")
    now = datetime.now(timezone.utc).isoformat()

    defaults = {
        BUFFER_FILE: "",
        CONTEXT_MAP_FILE: json.dumps({"nodes": [], "edges": [], "last_updated": now}, indent=2),
        CURRENT_CONTEXT_FILE: json.dumps({
            "timestamp": now,
            "active_project": None,
            "jira_key": None,
            "inferred_task": None,
            "confidence": 0.0,
            "trigger_jira_sync": False,
            "tags": [],
        }, indent=2),
        JIRA_STATE_FILE: json.dumps({"tickets": {}, "last_sync": None, "projects": []}, indent=2),
        TICKET_MAPPINGS_FILE: json.dumps({}, indent=2),
    }

    for path, content in defaults.items():
        if not path.exists():
            path.write_text(content)
            print(f"  ✓ Created {path.name}")
        else:
            print(f"  · Exists  {path.name}")


def check_mcp_config() -> bool:
    config_path = HERMES_HOME / "config.yaml"
    if not config_path.exists():
        print(f"  ✗ config.yaml not found at {config_path}")
        return False
    if "screenpipe" in config_path.read_text():
        print("  ✓ Screenpipe MCP already in config.yaml")
        return True
    print("  ✗ Screenpipe not configured — run with --add-mcp to add it")
    return False


def add_mcp_config():
    try:
        import yaml
    except ImportError:
        print("  ✗ PyYAML not installed. Add manually to ~/.hermes/config.yaml:")
        print("""
mcp_servers:
  screenpipe:
    command: npx
    args: ["-y", "screenpipe-mcp"]
    timeout: 30
""")
        return

    config_path = HERMES_HOME / "config.yaml"
    with open(config_path) as f:
        config = yaml.safe_load(f) or {}

    mcp = config.setdefault("mcp_servers", {})
    if "screenpipe" not in mcp:
        mcp["screenpipe"] = {"command": "npx", "args": ["-y", "screenpipe-mcp"], "timeout": 30}
        with open(config_path, "w") as f:
            yaml.dump(config, f, default_flow_style=False, allow_unicode=True, sort_keys=False)
        print("  ✓ Added screenpipe to config.yaml")
    else:
        print("  ✓ Screenpipe already configured")


def main():
    print("=== Activity Intelligence Bootstrap ===\n")

    init_directories()
    init_state_files()

    print("\nChecking Screenpipe MCP...")
    if "--add-mcp" in sys.argv:
        add_mcp_config()
    else:
        check_mcp_config()

    print("\n✓ Bootstrap complete.")
    print("\nNext steps:")
    print("  1. Ensure Screenpipe is running (open Screenpipe app)")
    print("  2. Start the agent system:")
    print("       cd /path/to/hermes-agent && python -m agents.orchestrator")


if __name__ == "__main__":
    main()
