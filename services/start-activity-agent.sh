#!/usr/bin/env bash
# Launcher for the Activity Intelligence agent system.
# Uses the hermes virtualenv Python which has all dependencies.

VENV_PYTHON="${HOME}/.hermes/hermes-agent/venv/bin/python"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [[ ! -x "$VENV_PYTHON" ]]; then
  echo "Error: hermes venv not found at $VENV_PYTHON"
  exit 1
fi

cd "$SCRIPT_DIR"
exec "$VENV_PYTHON" -m agents.orchestrator "$@"
