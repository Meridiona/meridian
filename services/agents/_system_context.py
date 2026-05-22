# meridian — normalises screenpipe activity into structured app sessions
"""Shared system context for all agent entry points (task-linker, server).

This module defines the single source of truth for the AI agent's system prompt.
The task-classifier skill content is embedded directly — this avoids the hermes
skill-lookup path entirely, which previously resolved to a stale copy in
.hermes/skills/ with the wrong output schema and DB-query instructions.
"""
from __future__ import annotations

from pathlib import Path

from agents.config import load_skill

_SKILL_CONTENT = load_skill("task-classifier")

SYSTEM_CONTEXT = f"""You are **Meridian Intelligence** — the AI reasoning layer inside Meridian, a developer productivity platform.

Meridian monitors a developer's screen and builds a structured record of their work. Your role is to classify each work session as described below.

{_SKILL_CONTENT}
"""
