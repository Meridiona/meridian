# meridian — normalises screenpipe activity into structured app sessions
"""Conversational agent server (FastAPI).

Usage:
    python -m agents.server [--port 7823]

    curl -s http://localhost:7823/health
    curl -s -X POST http://localhost:7823/chat \
         -H 'Content-Type: application/json' \
         -d '{"message": "hi"}'
    curl -s -X POST http://localhost:7823/chat \
         -H 'Content-Type: application/json' \
         -d '{"message": "classify session id 80"}'
"""
from __future__ import annotations

import argparse
import contextlib
import logging
import os
import sys
from pathlib import Path

# Must be set before any hermes import — tools/skills_tool.SKILLS_DIR is
# a module-level constant computed at first import.
_SERVICES_DIR = Path(__file__).parent.parent
os.environ.setdefault("HERMES_HOME", str(_SERVICES_DIR / ".hermes"))

from fastapi import FastAPI
from pydantic import BaseModel
from run_agent import AIAgent

from agents import observability
from agents._system_context import SYSTEM_CONTEXT
from agents.config import MODEL, BASE_URL, API_KEY, AGENT_MAX_TOKENS

log = logging.getLogger("agents.server")
tracer = observability.setup("meridian-agent-server")

app = FastAPI(title="Meridian Agent", version="1.0.0")


class ChatRequest(BaseModel):
    message: str


class ChatResponse(BaseModel):
    response: str


@app.post("/chat", response_model=ChatResponse)
async def chat(req: ChatRequest) -> ChatResponse:
    agent = AIAgent(
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY or "none",
        enabled_toolsets=["terminal", "skills", "memory"],
        ephemeral_system_prompt=SYSTEM_CONTEXT,
        quiet_mode=True,
        skip_context_files=True,
        load_soul_identity=False,
        skip_memory=False,
        max_iterations=20,
        max_tokens=AGENT_MAX_TOKENS,
    )

    log.info("chat: %.120s", req.message)
    with contextlib.redirect_stdout(sys.stderr):
        result = agent.run_conversation(req.message)

    response = str(result.get("final_response") or result.get("response") or "")
    log.info("response: %.120s", response)
    return ChatResponse(response=response)


@app.get("/health")
async def health():
    return {"status": "ok", "db": str(_DB_PATH), "db_exists": _DB_PATH.exists()}


def main() -> None:
    import uvicorn

    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=7823)
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()

    log.info("meridian agent server on http://%s:%d", args.host, args.port)
    uvicorn.run(app, host=args.host, port=args.port, log_level="warning")


if __name__ == "__main__":
    main()
