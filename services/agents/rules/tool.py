"""Tool-dimension rules — multi-value, open vocabulary.

We map app names to canonical tool slugs and pick out specific tool/service
hosts from URL extracts. Slugs are stable so they aggregate cleanly across
sessions in user_profile rollups.
"""
from __future__ import annotations

import re

from agents.rules import RuleHit, rule, extract_urls, session_text

# Map raw app names to a small canonical slug set.
APP_TO_TOOL = {
    "Code":              "vscode",
    "Cursor":            "cursor",
    "Xcode":             "xcode",
    "PyCharm":           "pycharm",
    "IntelliJ IDEA":     "intellij",
    "WebStorm":          "webstorm",
    "Sublime Text":      "sublime",
    "Zed":               "zed",
    "Terminal":          "terminal",
    "iTerm2":            "iterm2",
    "Warp":              "warp",
    "Ghostty":           "ghostty",
    "Slack":             "slack",
    "Microsoft Teams":   "teams",
    "Discord":           "discord",
    "Mail":              "mail",
    "Outlook":           "outlook",
    "Messages":          "messages",
    "Notes":             "notes",
    "Obsidian":          "obsidian",
    "Notion":            "notion",
    "Figma":             "figma",
    "Sketch":            "sketch",
    "Linear":            "linear",
    "Claude":            "claude.ai",
    "ChatGPT":           "chatgpt",
    "Ollama":            "ollama",
    "Gemini":            "gemini",
    "Perplexity":        "perplexity",
    "Google Chrome":     "chrome",
    "Safari":            "safari",
    "Arc":               "arc",
    "Firefox":           "firefox",
}

# Specific hosts → tool slug.
URL_HOSTS = [
    (re.compile(r"\bclaude\.ai\b", re.I),        "claude.ai"),
    (re.compile(r"\bchat\.openai\.com\b", re.I), "chatgpt"),
    (re.compile(r"\bgemini\.google\.com\b", re.I), "gemini"),
    (re.compile(r"\bperplexity\.ai\b", re.I),    "perplexity"),
    (re.compile(r"\bgithub\.com\b", re.I),       "github"),
    (re.compile(r"\bgitlab\.com\b", re.I),       "gitlab"),
    (re.compile(r"\.atlassian\.net\b", re.I),    "jira"),
    (re.compile(r"\bnotion\.so\b", re.I),        "notion"),
    (re.compile(r"\blinear\.app\b", re.I),       "linear"),
    (re.compile(r"\bfigma\.com\b", re.I),        "figma"),
    (re.compile(r"\baws\.amazon\.com\b", re.I),  "aws"),
    (re.compile(r"\bvercel\.com\b", re.I),       "vercel"),
    (re.compile(r"\bnetlify\.com\b", re.I),      "netlify"),
    (re.compile(r"\bdocs\.rs\b", re.I),          "rust-docs"),
    (re.compile(r"\bcrates\.io\b", re.I),        "crates.io"),
    (re.compile(r"\bnpmjs\.com\b", re.I),        "npm-registry"),
    (re.compile(r"\bpypi\.org\b", re.I),         "pypi"),
    (re.compile(r"\bstackoverflow\.com\b", re.I), "stackoverflow"),
]

# CLI tools / processes named in OCR or terminal titles.
CLI_TOOL_RE = re.compile(
    r"\b(cargo|npm|pnpm|yarn|pip|uv|uvx|poetry|"
    r"docker|kubectl|terraform|ansible|helm|"
    r"git|gh|jj|"
    r"node|deno|bun|"
    r"sqlite3|psql|redis-cli|"
    r"pytest|jest|vitest|cypress|playwright|"
    r"clippy|rustfmt|prettier|eslint|tsc|mypy|ruff|"
    r"cargo-test|cargo-clippy|cargo-fmt|"
    r"ollama)\b",
    re.I,
)


@rule(name="app_to_tool", dim="tool")
def _app_to_tool(session: dict):
    app = session.get("app_name") or ""
    slug = APP_TO_TOOL.get(app)
    if not slug:
        return None
    return RuleHit(
        dimension="tool",
        value=slug,
        confidence=0.95,
        explanation=f"app_name={app!r}",
    )


@rule(name="url_hosts_to_tools", dim="tool")
def _url_hosts(session: dict):
    text = session_text(session, ocr_limit=20)
    hits: list[RuleHit] = []
    seen: set[str] = set()
    for pattern, slug in URL_HOSTS:
        if slug in seen:
            continue
        if pattern.search(text):
            hits.append(RuleHit(
                dimension="tool",
                value=slug,
                confidence=0.9,
                explanation=f"url host matched {pattern.pattern!r}",
            ))
            seen.add(slug)
    return hits or None


@rule(name="cli_tools_in_ocr", dim="tool")
def _cli_tools(session: dict):
    text = session_text(session, ocr_limit=15)
    found: dict[str, int] = {}
    for m in CLI_TOOL_RE.finditer(text):
        slug = m.group(1).lower()
        found[slug] = found.get(slug, 0) + 1
    if not found:
        return None
    hits: list[RuleHit] = []
    for slug, n in found.items():
        # Confidence rises with frequency, capped.
        conf = min(0.6 + 0.05 * n, 0.85)
        hits.append(RuleHit(
            dimension="tool",
            value=slug,
            confidence=conf,
            explanation=f"{n}× '{slug}' in OCR",
        ))
    return hits
