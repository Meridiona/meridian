"""Topic-dimension rules — multi-value, open vocabulary.

Stage 1 is keyword-based. Stage 2 will replace this with embedding-similarity
to a topic-anchor list. For now we ship a curated set of high-signal regexes
that catch the topics this user actually works on (Rust / async / sqlite /
embeddings / ...) — extend over time as you notice common terms.
"""
from __future__ import annotations

import re

from agents.rules import RuleHit, rule, session_text

# Each entry: (slug, regex). Regex is matched case-insensitively against the
# joined session text. Be conservative — false positives clutter user_profile.
TOPIC_PATTERNS = [
    ("rust",              re.compile(r"\b(rust(?:lang)?|cargo|clippy|rustc|borrow checker)\b", re.I)),
    ("typescript",        re.compile(r"\b(typescript|tsc|ts-node|tsconfig)\b", re.I)),
    ("python",            re.compile(r"\b(python|pip|venv|pytest|ruff|mypy|cpython)\b", re.I)),
    ("react",             re.compile(r"\b(react|jsx|tsx|next\.js|vite)\b", re.I)),
    ("nextjs",            re.compile(r"\bnext\.?js\b", re.I)),
    ("sqlite",            re.compile(r"\b(sqlite3?|sqlx|better-sqlite3|sqlite-vec)\b", re.I)),
    ("postgres",          re.compile(r"\b(postgres|postgresql|psql|pgvector)\b", re.I)),
    ("async",             re.compile(r"\b(async\s*(?:fn|def|function)?|await|tokio|asyncio|future\b|promise)\b", re.I)),
    ("embeddings",        re.compile(r"\b(embedding|vector(?:store)?|cosine similarity|sentence[- ]transformers|"
                                     r"all-MiniLM|faiss|qdrant)\b", re.I)),
    ("llm",               re.compile(r"\b(llm|gpt-?\d|claude(?:-\d)?|gemini|gemma|nemotron|"
                                     r"sonnet|haiku|opus|tool[- ]call(?:ing)?|prompt cache)\b", re.I)),
    ("mcp",               re.compile(r"\bmcp\b|model[- ]context[- ]protocol", re.I)),
    ("screenpipe",        re.compile(r"\bscreenpipe\b", re.I)),
    ("meridian",          re.compile(r"\bmeridian(?:a|-v\d)?\b", re.I)),
    ("hermes",            re.compile(r"\bhermes(?:-?agent)?\b", re.I)),
    ("ollama",            re.compile(r"\bollama(?:[- ]cloud)?\b", re.I)),
    ("jira",              re.compile(r"\b(jira|atlassian|kanban|sprint)\b", re.I)),
    ("github",            re.compile(r"\b(github|pull request|pr review|gh cli)\b", re.I)),
    ("docker",            re.compile(r"\b(docker|dockerfile|compose|container image)\b", re.I)),
    ("kubernetes",        re.compile(r"\b(kubernetes|kubectl|k8s|helm|kustomize)\b", re.I)),
    ("aws",               re.compile(r"\b(aws|amazon web services|s3 bucket|lambda function|cloudwatch)\b", re.I)),
    ("git",               re.compile(r"\b(git\s+(commit|push|pull|rebase|stash)|merge conflict)\b", re.I)),
    ("auth",              re.compile(r"\b(oauth|jwt|session token|api key|api token|"
                                     r"single sign-on|sso\b)", re.I)),
    ("observability",     re.compile(r"\b(observability|tracing|opentelemetry|prometheus|grafana|"
                                     r"structured logging)\b", re.I)),
    ("vector_search",     re.compile(r"\b(vector search|ann index|hnsw|sqlite-vec|pgvector|qdrant|weaviate|milvus)\b", re.I)),
    ("ai_agents",         re.compile(r"\b(ai[- ]agent|agentic|tool[- ]calling|agent loop|multi[- ]agent)\b", re.I)),
    ("tailscale",         re.compile(r"\btailscale\b", re.I)),
    ("remote_access",     re.compile(r"\b(remote desktop|screen sharing|vnc|teamviewer|anydesk)\b", re.I)),
]


@rule(name="topic_keywords", dim="topic")
def _topic_keywords(session: dict):
    text = session_text(session, ocr_limit=20)
    hits: list[RuleHit] = []
    for slug, pattern in TOPIC_PATTERNS:
        m = pattern.search(text)
        if not m:
            continue
        # Confidence rises with multiple distinct mentions.
        n = len(pattern.findall(text))
        conf = min(0.55 + 0.05 * (n - 1), 0.85)
        hits.append(RuleHit(
            dimension="topic",
            value=slug,
            confidence=conf,
            explanation=f"{n}× '{slug}' keyword",
        ))
    return hits or None
