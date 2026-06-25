"""Agno agent factories for the worklog pipeline.

All reasoning agents talk to the local MLX server's OpenAI-compatible endpoint
(`OpenAILike` → `/v1/chat/completions`), which FSM-constrains decoding to the
agent's ``output_schema`` via outlines. Schema enforcement, no regex.
"""
from __future__ import annotations

from typing import Any, Callable

_DEFAULT_SERVER = "http://127.0.0.1:7823"


def _model(server_url: str, max_tokens: int, temperature: float) -> Any:
    from agno.models.openai.like import OpenAILike

    base = server_url.rstrip("/")
    if not base.endswith("/v1"):
        base = f"{base}/v1"
    return OpenAILike(
        id="qwen3.5-2b",
        base_url=base,
        api_key="local",
        max_tokens=max_tokens,
        temperature=temperature,
        request_params={"timeout": 300},
    )


def make_match_agent_factory(
    system: str,
    server_url: str = _DEFAULT_SERVER,
    max_tokens: int = 2048,
    temperature: float = 0.0,
) -> Callable[[type], Any]:
    """Return ``factory(output_schema) -> Agent`` for the tiered matcher.

    A fresh Agent per call so each tier/batch gets its own candidate-constrained
    schema. Construction is cheap — the model is a remote HTTP client, not a load.
    """
    from agno.agent import Agent

    def factory(output_schema: type) -> Any:
        agent = Agent(
            model=_model(server_url, max_tokens, temperature),
            output_schema=output_schema,
            use_json_mode=False,
            add_history_to_context=False,
            markdown=False,
        )
        agent.system_message = system
        return agent

    return factory


def make_schema_agent(
    system: str,
    output_schema: type,
    server_url: str = _DEFAULT_SERVER,
    max_tokens: int = 1024,
    temperature: float = 0.0,
) -> Any:
    """A single schema-enforced agent (propose-ticket, worklog)."""
    from agno.agent import Agent

    agent = Agent(
        model=_model(server_url, max_tokens, temperature),
        output_schema=output_schema,
        use_json_mode=False,
        add_history_to_context=False,
        markdown=False,
    )
    agent.system_message = system
    return agent
