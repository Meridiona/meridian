# meridian — normalises screenpipe activity into structured app sessions

"""Tests for meridian_agents.llm.

Pure-helper tests run unconditionally. The live Cloud Ollama smoke test
is opt-in: set MERIDIAN_AGENTS_LIVE_LLM=1 plus real OLLAMA_API_KEY /
OLLAMA_MODEL to exercise it.
"""

from __future__ import annotations

import os
from dataclasses import replace

import pytest

from meridian_agents.config import Config, JiraConfig
from meridian_agents.llm import (
    DEFAULT_MAX_ITERATIONS,
    DEFAULT_SMOKE_PROMPT,
    agent_kwargs,
    chat,
    resolve_ollama_base_url,
)


def _fake_config(**overrides) -> Config:
    base = Config(
        meridian_db="/tmp/x.db",
        poll_interval_secs=300,
        auto_threshold=0.85,
        queue_threshold=0.6,
        log_filter="meridian_agents=info",
        ollama_base_url="https://ollama.com",
        ollama_api_key="ollama-test-key",
        ollama_model="gpt-oss:120b",
        jira=None,
    )
    return replace(base, **overrides)


# ---------------------------------------------------------------------------
# resolve_ollama_base_url
# ---------------------------------------------------------------------------


def test_resolve_appends_v1_when_missing():
    assert resolve_ollama_base_url("https://ollama.com") == "https://ollama.com/v1"


def test_resolve_strips_trailing_slash():
    assert resolve_ollama_base_url("https://ollama.com/") == "https://ollama.com/v1"


def test_resolve_idempotent_when_already_v1():
    assert resolve_ollama_base_url("https://ollama.com/v1") == "https://ollama.com/v1"


def test_resolve_idempotent_when_v1_with_trailing_slash():
    assert resolve_ollama_base_url("https://ollama.com/v1/") == "https://ollama.com/v1"


def test_resolve_handles_custom_endpoint():
    assert (
        resolve_ollama_base_url("http://localhost:11434")
        == "http://localhost:11434/v1"
    )


# ---------------------------------------------------------------------------
# agent_kwargs
# ---------------------------------------------------------------------------


def test_agent_kwargs_resolves_base_url():
    cfg = _fake_config()
    kw = agent_kwargs(cfg, system_prompt="hi")
    assert kw["base_url"] == "https://ollama.com/v1"


def test_agent_kwargs_passes_credentials_and_model():
    cfg = _fake_config(
        ollama_api_key="ollama-real-key",
        ollama_model="qwen3-coder:480b",
    )
    kw = agent_kwargs(cfg, system_prompt="hi")
    assert kw["api_key"] == "ollama-real-key"
    assert kw["model"] == "qwen3-coder:480b"


def test_agent_kwargs_passes_system_prompt_as_ephemeral():
    cfg = _fake_config()
    kw = agent_kwargs(cfg, system_prompt="you are the synthesizer")
    assert kw["ephemeral_system_prompt"] == "you are the synthesizer"


def test_agent_kwargs_disables_built_in_toolsets():
    cfg = _fake_config()
    kw = agent_kwargs(cfg, system_prompt="hi")
    assert kw["enabled_toolsets"] == []


def test_agent_kwargs_skips_hermes_context_files():
    cfg = _fake_config()
    kw = agent_kwargs(cfg, system_prompt="hi")
    assert kw["skip_context_files"] is True
    assert kw["load_soul_identity"] is False
    assert kw["skip_memory"] is True


def test_agent_kwargs_runs_quiet_for_daemon_use():
    cfg = _fake_config()
    kw = agent_kwargs(cfg, system_prompt="hi")
    assert kw["quiet_mode"] is True
    assert kw["verbose_logging"] is False
    assert kw["save_trajectories"] is False


def test_agent_kwargs_defaults_max_iterations():
    cfg = _fake_config()
    kw = agent_kwargs(cfg, system_prompt="hi")
    assert kw["max_iterations"] == DEFAULT_MAX_ITERATIONS


def test_agent_kwargs_respects_max_iterations_override():
    cfg = _fake_config()
    kw = agent_kwargs(cfg, system_prompt="hi", max_iterations=3)
    assert kw["max_iterations"] == 3


def test_agent_kwargs_zero_tool_delay():
    cfg = _fake_config()
    kw = agent_kwargs(cfg, system_prompt="hi")
    assert kw["tool_delay"] == 0.0


def test_agent_kwargs_includes_jira_independent_fields():
    """Jira config has no business in agent_kwargs — that's a sink-side concern."""
    cfg = _fake_config(
        jira=JiraConfig(
            base_url="https://example.atlassian.net",
            email="x@y.z",
            api_token="t",
        )
    )
    kw = agent_kwargs(cfg, system_prompt="hi")
    flat = " ".join(f"{k}={v}" for k, v in kw.items())
    assert "atlassian" not in flat.lower()
    assert "jira" not in flat.lower()


# ---------------------------------------------------------------------------
# DEFAULT_SMOKE_PROMPT — sanity check
# ---------------------------------------------------------------------------


def test_default_smoke_prompt_is_short():
    assert len(DEFAULT_SMOKE_PROMPT) < 200
    assert DEFAULT_SMOKE_PROMPT.strip() != ""


# ---------------------------------------------------------------------------
# Live integration smoke test (opt-in)
# ---------------------------------------------------------------------------


_LIVE = os.environ.get("MERIDIAN_AGENTS_LIVE_LLM") == "1"


@pytest.mark.skipif(
    not _LIVE,
    reason=(
        "live LLM smoke test — set MERIDIAN_AGENTS_LIVE_LLM=1 plus a real "
        "OLLAMA_API_KEY/OLLAMA_MODEL to enable. This test makes a real "
        "network request to Cloud Ollama and costs tokens."
    ),
)
async def test_chat_against_real_cloud_ollama():
    """Smoke test against the actual Cloud Ollama endpoint.

    Skipped by default. Run with:
        MERIDIAN_AGENTS_LIVE_LLM=1 OLLAMA_API_KEY=... OLLAMA_MODEL=... \\
            uv run pytest tests/test_llm.py::test_chat_against_real_cloud_ollama -v -s
    """
    from meridian_agents.config import load

    cfg = load(dotenv_path="")  # require env vars to be set explicitly
    response = await chat(cfg, "Reply with the single word 'pong' and nothing else.")
    assert response  # any non-empty response means we connected
