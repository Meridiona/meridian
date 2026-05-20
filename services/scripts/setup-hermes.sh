#!/usr/bin/env bash
# meridian — one-time hermes gateway setup
#
# Creates services/.hermes/ from templates, substituting the real absolute path
# for __SKILLS_DIR__. Safe to re-run — only overwrites config.yaml, never .env.
#
# Usage:
#   cd meridian/services
#   bash scripts/setup-hermes.sh

set -euo pipefail

SERVICES_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HERMES_HOME="${SERVICES_DIR}/.hermes"
TEMPLATE_DIR="${SERVICES_DIR}/hermes-config"
SKILLS_DIR="${SERVICES_DIR}/skills/activity"

echo "Setting up hermes home at: ${HERMES_HOME}"

mkdir -p "${HERMES_HOME}"

# Generate config.yaml with real path substituted
sed "s|__SKILLS_DIR__|${SKILLS_DIR}|g" \
    "${TEMPLATE_DIR}/config.yaml" > "${HERMES_HOME}/config.yaml"
echo "  ✓ config.yaml written"

# Create .env from template only if it does not already exist
if [ ! -f "${HERMES_HOME}/.env" ]; then
    cp "${TEMPLATE_DIR}/.env.template" "${HERMES_HOME}/.env"
    echo "  ✓ .env created from template — fill in your OLLAMA_API_KEY"
else
    echo "  ✓ .env already exists — skipping (not overwritten)"
fi

# Create memories dir so hermes finds it on first run
mkdir -p "${HERMES_HOME}/memories"
echo "  ✓ memories/ ready"

echo ""
echo "Done. Next steps:"
echo "  1. Edit services/.hermes/.env and set OLLAMA_API_KEY"
echo "  2. Install: cd services && pip install -e ."
echo "  3. Smoke test: echo '{\"sessions\":[],\"pm_tasks\":[]}' | HERMES_HOME=${HERMES_HOME} python -m agents.run_task_linker"
