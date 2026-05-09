#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
set -e

git config core.hooksPath .githooks
chmod +x .githooks/pre-commit
chmod +x .githooks/commit-msg
chmod +x .githooks/pre-push

echo "Git hooks installed:"
echo "  commit-msg  — conventional commits format check"
echo "  pre-commit  — cargo fmt + clippy"
echo "  pre-push    — cargo fmt + clippy + cargo test + UI build + UI tests"
echo ""
echo "Commit format: feat|fix|docs|refactor|perf|chore|ci(scope): description"
