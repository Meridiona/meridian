# Skills

Claude Code skills for the Meridian repo. Skills are reusable, context-aware instructions for common workflows.

Skills live in `.claude/skills/` and can be invoked in Claude Code.

## Available Skills

| Skill | File | Use When |
|-------|------|----------|
| `release` | `.claude/skills/release/SKILL.md` | Bumping versions, triggering builds, monitoring CI |
| `meridian-etl` | `.claude/skills/meridian-etl/SKILL.md` | Debugging ETL pipeline, inspecting sessions, fixing boundary issues |
| `meridian-mcp` | `.claude/skills/meridian-mcp/SKILL.md` | Building, configuring, and debugging the MCP server |

## Quick Reference

```bash
# Debug ETL — verbose logging
RUST_LOG=debug ./target/release/meridian

# Query the DB directly
sqlite3 ~/.meridian/meridian.db "SELECT app_name, ROUND(SUM(duration_s)/60,1) AS min FROM app_sessions GROUP BY app_name ORDER BY min DESC LIMIT 10;"

# Build MCP server
cd packages/meridian-mcp && npm run build

# Run tests
cargo test
```
