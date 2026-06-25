#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
set -e

git config core.hooksPath .githooks
chmod +x .githooks/pre-commit
chmod +x .githooks/commit-msg
chmod +x .githooks/pre-push
chmod +x .githooks/post-push

echo "Git hooks installed:"
echo "  commit-msg  — conventional commits format check"
echo "  pre-commit  — cargo fmt + clippy"
echo "  pre-push    — cargo fmt + clippy + UI build + UI tests + security audit + cargo test"
echo "  post-push   — sync services/observability/dashboards/ → local OpenObserve"
echo ""
echo "Commit format: feat|fix|docs|refactor|perf|chore|ci(scope): description"
