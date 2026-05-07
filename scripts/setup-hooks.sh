#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
set -e

git config core.hooksPath .githooks
chmod +x .githooks/pre-commit
chmod +x .githooks/commit-msg

echo "Git hooks installed:"
echo "  pre-commit  — cargo fmt + clippy"
echo "  commit-msg  — conventional commits format check"
echo ""
echo "Commit format: feat|fix|docs|refactor|perf|chore|ci(scope): description"
