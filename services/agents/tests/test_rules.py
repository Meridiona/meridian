# meridian — normalises screenpipe activity into structured app sessions
"""Unit tests for the Stage-1 rule library.

Covers registry discovery, end-to-end run_rules + resolve_hits, ticket and
text helpers, and the single-/multi-value dimension policy.
"""
from __future__ import annotations

import pytest

from agents.rules import (
    RULE_REGISTRY,
    RuleHit,
    discover_rules,
    extract_tickets,
    resolve_hits,
    run_rules,
    session_text,
)
from agents import text_for_embedding as tfe


@pytest.fixture(autouse=True)
def _ensure_rules_discovered():
    """Make sure every rule submodule is registered before each test."""
    discover_rules()
    yield


def _by_dim(hits, dim):
    return [h for h in hits if h.dimension == dim]


def test_discover_rules_registers_many_rules():
    """discover_rules wires up the full registry (>30 rules across submodules)."""
    discover_rules()
    assert len(RULE_REGISTRY) > 30, (
        f"expected >30 registered rules, got {len(RULE_REGISTRY)}"
    )
    # Each entry is a (name, dim, callable) tuple.
    for name, dim, fn in RULE_REGISTRY:
        assert isinstance(name, str) and name
        assert isinstance(dim, str) and dim
        assert callable(fn)


def test_coding_session_round_trip(make_session):
    """A VS Code session over a .py file produces activity=coding + tool=vscode."""
    sess = make_session(
        id=42,
        app_name="Code",
        duration_s=900,            # 15 min → engagement=deep_work
        window_titles=[{"window_name": "main.py — meridian", "count": 5}],
        ocr_samples=[{"text": "def run_etl(): pass  # main.py"}],
    )
    hits = resolve_hits(run_rules(sess))

    activities = {h.value for h in _by_dim(hits, "activity")}
    tools      = {h.value for h in _by_dim(hits, "tool")}
    engagements = {h.value for h in _by_dim(hits, "engagement")}

    assert "coding" in activities
    assert "vscode" in tools
    # 15 min duration → deep_work
    assert engagements == {"deep_work"}


def test_ai_chat_app_with_prompt_vocab(make_session):
    """AI chat-app sessions with prompt-engineering tokens emit the right activity."""
    sess = make_session(
        id=1,
        app_name="ChatGPT",
        duration_s=300,
        window_titles=[{"window_name": "ChatGPT — system prompt", "count": 3}],
        ocr_samples=[{"text": "you are an expert. system prompt: respond in JSON mode."}],
    )
    hits = resolve_hits(run_rules(sess))
    activities = {h.value for h in _by_dim(hits, "activity")}
    collabs    = {h.value for h in _by_dim(hits, "collaboration")}

    assert "prompt_engineering" in activities
    assert "ai_assisted" in collabs


def test_topic_keywords_picks_up_multiple(make_session):
    """topic_keywords emits one hit per matched topic regex."""
    sess = make_session(
        id=2,
        app_name="Code",
        duration_s=600,
        ocr_samples=[
            {"text": "rust + cargo clippy + sqlx + tokio async fn"},
            {"text": "embeddings via sentence-transformers, faiss index"},
        ],
    )
    hits = run_rules(sess)
    topic_values = {h.value for h in hits if h.dimension == "topic"}
    # Several known topics should fire.
    assert {"rust", "sqlite", "async", "embeddings"} <= topic_values


def test_ticket_keys_in_text_emits_topic_and_skips_denylist(make_session):
    """Verbatim KAN-86 mentions become topic hits; UTF-8 / GPT-4 do not."""
    sess = make_session(
        id=3,
        app_name="Code",
        ocr_samples=[
            {"text": "working on KAN-86 — encoding UTF-8 with GPT-4 helper"},
        ],
    )
    hits = run_rules(sess)
    topic_values = {h.value for h in hits if h.dimension == "topic"}
    assert "KAN-86" in topic_values
    assert "UTF-8" not in topic_values
    assert "GPT-4" not in topic_values


def test_single_value_resolution_picks_highest_confidence():
    """Two activity hits resolve to just the higher-confidence one."""
    raw = [
        RuleHit(dimension="activity", value="coding",   confidence=0.4),
        RuleHit(dimension="activity", value="learning", confidence=0.75),
    ]
    resolved = resolve_hits(raw)
    activities = _by_dim(resolved, "activity")
    assert len(activities) == 1
    assert activities[0].value == "learning"


def test_multi_value_dimensions_keep_all_distinct_values():
    """Tool dimension is multi-value: vscode + github both survive."""
    raw = [
        RuleHit(dimension="tool", value="vscode", confidence=0.95),
        RuleHit(dimension="tool", value="github", confidence=0.9),
    ]
    resolved = resolve_hits(raw)
    tool_values = {h.value for h in _by_dim(resolved, "tool")}
    assert tool_values == {"vscode", "github"}


def test_extract_tickets_dedupes(make_session):
    """extract_tickets dedupes duplicate keys in title and OCR."""
    sess = make_session(
        ocr_samples=[
            {"text": "KAN-86 first mention, KAN-86 second mention, also KAN-99"},
        ],
        window_titles=[{"window_name": "feat/KAN-86-migrate", "count": 1}],
    )
    keys = extract_tickets(sess)
    assert keys.count("KAN-86") == 1
    assert "KAN-99" in keys


def test_session_text_weights_titles_by_count():
    """text_for_embedding._join_titles repeats by count, capped at 3×."""
    sess = {
        "app_name": "Code",
        "window_titles": [
            {"window_name": "main.py", "count": 10},   # capped to 3×
            {"window_name": "Cargo.toml", "count": 1},
        ],
        "ocr_samples": [],
        "audio_snippets": [],
    }
    text = tfe.session_text(sess)
    # main.py should appear 3× (cap) and Cargo.toml exactly once.
    assert text.count("main.py") == 3
    assert text.count("Cargo.toml") == 1


def test_vscode_extension_banner_stripped_from_titles():
    """The VS Code "extensions want to relaunch" banner is removed before tokenisation."""
    sess = {
        "app_name": "Code",
        "window_titles": [
            {
                "window_name":
                    "main.py — The following extensions want to relaunch the terminal because"
                    " they have updated: Python, GitHub Copilot",
                "count": 1,
            },
        ],
        "ocr_samples": [],
        "audio_snippets": [],
    }
    text = tfe.session_text(sess)
    # main.py keeps its place; the banner tail does not.
    assert "main.py" in text
    assert "extensions want to relaunch" not in text
    # The polluting tokens that the banner used to inject are gone.
    assert "GitHub Copilot" not in text
    assert "Python," not in text
