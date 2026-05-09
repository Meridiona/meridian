# meridian — normalises screenpipe activity into structured app sessions

"""Thin async wrapper around hermes' AIAgent.

Configures hermes for Cloud Ollama (OpenAI-compatible endpoint, no
auto-loaded SOUL.md / AGENTS.md, no built-in toolsets). Exposes:

- `make_agent(cfg, system_prompt)`   — construct a configured `AIAgent`
- `chat(cfg, message)`               — send one message, return final text
                                       (no tools — smoke-test path)
- `run_conversation(cfg, user_message, system_prompt)` — full tool-call
                                       loop; the synthesizer uses this
                                       after registering its tools.

Tool registration (write_session_summary, match_session_to_task,
upsert_context_node, write_current_context) lives in
`agents/synthesizer.py` so it stays adjacent to the SKILL.md prompt
those tools satisfy.
"""

from __future__ import annotations

import asyncio
from typing import Any

from run_agent import AIAgent

from meridian_agents.config import Config

DEFAULT_SMOKE_PROMPT = (
    "You are a helpful assistant. Answer in one sentence."
)
DEFAULT_MAX_ITERATIONS = 20


def resolve_ollama_base_url(base_url: str) -> str:
    """Cloud Ollama serves its OpenAI-compatible endpoint at /v1.

    The user-facing config var (`OLLAMA_BASE_URL`) defaults to
    `https://ollama.com`; hermes' OpenAI client wants the `/v1` suffix.
    Idempotent: leaves an already-suffixed URL alone.
    """
    cleaned = base_url.rstrip("/")
    if cleaned.endswith("/v1"):
        return cleaned
    return f"{cleaned}/v1"


def agent_kwargs(
    cfg: Config,
    *,
    system_prompt: str,
    max_iterations: int = DEFAULT_MAX_ITERATIONS,
) -> dict[str, Any]:
    """Build the kwargs dict for `AIAgent(**kwargs)`.

    Pulled out as its own function so tests can verify the meridian-side
    configuration without actually constructing an AIAgent (the constructor
    sets up hermes' DB session, tool registry filtering, etc.).
    """
    return dict(
        base_url=resolve_ollama_base_url(cfg.ollama_base_url),
        api_key=cfg.ollama_api_key,
        model=cfg.ollama_model,
        # The skill prompt drives behaviour. Ephemeral so it isn't saved
        # to trajectories — we don't want hermes' trajectory log either way.
        ephemeral_system_prompt=system_prompt,
        # No browser, terminal, file, web — synthesizer registers its own
        # tools under a dedicated toolset name and enables it explicitly.
        enabled_toolsets=[],
        # Don't auto-inject SOUL.md / AGENTS.md / .cursorrules from the cwd.
        # We control the system prompt.
        skip_context_files=True,
        load_soul_identity=False,
        skip_memory=True,
        # Daemon use — no terminal UI noise, no trajectory files on disk.
        quiet_mode=True,
        verbose_logging=False,
        save_trajectories=False,
        # Per-tick limits — bounded so a confused LLM can't loop forever.
        max_iterations=max_iterations,
        tool_delay=0.0,
    )


def make_agent(
    cfg: Config,
    *,
    system_prompt: str,
    max_iterations: int = DEFAULT_MAX_ITERATIONS,
) -> AIAgent:
    """Construct a hermes `AIAgent` configured for meridian + Cloud Ollama."""
    return AIAgent(
        **agent_kwargs(cfg, system_prompt=system_prompt, max_iterations=max_iterations)
    )


async def chat(
    cfg: Config,
    message: str,
    *,
    system_prompt: str = DEFAULT_SMOKE_PROMPT,
    max_iterations: int = 5,
) -> str:
    """Smoke-test path: send `message`, get the final text response.

    No tools. Used to verify the LLM connection (Cloud Ollama key, model
    name, base URL) before layering on the synthesizer's tool-call dance.
    """
    agent = make_agent(cfg, system_prompt=system_prompt, max_iterations=max_iterations)
    # `chat` and `run_conversation` are sync — wrap in a thread so we
    # don't block the orchestrator's asyncio loop.
    return await asyncio.to_thread(agent.chat, message)


async def run_conversation(
    cfg: Config,
    user_message: str,
    *,
    system_prompt: str,
    max_iterations: int = DEFAULT_MAX_ITERATIONS,
) -> dict[str, Any]:
    """Full agent run with tool calling. Returns hermes' result dict.

    The synthesizer calls this after registering its four Python tools.
    Result keys include `final_response`, `messages` (full history), and
    others — see hermes' `AIAgent.run_conversation` docstring.
    """
    agent = make_agent(cfg, system_prompt=system_prompt, max_iterations=max_iterations)
    return await asyncio.to_thread(agent.run_conversation, user_message)
