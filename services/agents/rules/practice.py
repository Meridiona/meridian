"""Practice-dimension rules — observable signals of "doing it well".

Multi-value, closed vocab. These flags answer: did the user write tests,
review code, run linters/type-checks, update docs, etc.
"""
from __future__ import annotations

import re

from agents.rules import RuleHit, rule, session_text, extract_urls

TEST_FILE_RE = re.compile(
    r"\b\S*(?:tests?|spec)[/_-]\S*\.(rs|py|ts|tsx|js|jsx|go|java|kt|rb)\b|"
    r"\b\S*\.(test|spec)\.(rs|py|ts|tsx|js|jsx|go|java|kt|rb)\b|"
    r"\btests?/\S+",
    re.I,
)
TYPE_CHECK_RE = re.compile(
    r"\b(clippy::|tsc\b|mypy\b|ruff\b|cargo[- ]check\b|"
    r"type[- ]check|TypeScript Error|Cannot find name|"
    r"expected .* found|borrowck|warning:.*unused)",
    re.I,
)
ERROR_HANDLING_RE = re.compile(
    r"\b(try\s*\{|except\s+\w+|catch\s*\(|Result<|"
    r"\.unwrap_or|\.context\(|anyhow!|Err\(|raise\s+\w+|panic!)\b",
    re.I,
)
REFACTOR_RE = re.compile(
    r"\b(refactor|extract method|extract function|rename symbol|"
    r"inline variable|move function)",
    re.I,
)
SECURITY_RE = re.compile(
    r"\b(security|vulnerability|CVE-\d+|sanitiz|escape|injection|XSS|CSRF|"
    r"OWASP|sandbox|secret|credential|RBAC)",
    re.I,
)
LINTING_RE = re.compile(
    r"\b(clippy\b|eslint\b|prettier\b|ruff\b|black\b|rustfmt\b|gofmt\b|"
    r"pre-commit|husky)\b",
    re.I,
)
CI_RE = re.compile(
    r"\b(github actions|gitlab ci|circleci|jenkins|workflow run|"
    r"pull_request:|push:.*runs-on:)",
    re.I,
)


@rule(name="tests_visible", dim="practice")
def _tests_visible(session: dict):
    text = session_text(session, ocr_limit=15)
    matches = TEST_FILE_RE.findall(text)
    if not matches:
        return None
    return RuleHit(
        dimension="practice",
        value="tests_written",
        confidence=0.85,
        explanation=f"{len(matches)} test-file mention(s) visible",
    )


@rule(name="type_check_visible", dim="practice")
def _type_check_visible(session: dict):
    text = session_text(session, ocr_limit=15)
    if not TYPE_CHECK_RE.search(text):
        return None
    return RuleHit(
        dimension="practice",
        value="type_checking",
        confidence=0.8,
        explanation="type-checker output / annotations visible",
    )


@rule(name="error_handling_visible", dim="practice")
def _error_handling_visible(session: dict):
    text = session_text(session, ocr_limit=15)
    if not ERROR_HANDLING_RE.search(text):
        return None
    return RuleHit(
        dimension="practice",
        value="error_handling",
        confidence=0.7,
        explanation="error-handling constructs in OCR",
    )


@rule(name="refactor_signal", dim="practice")
def _refactor_signal(session: dict):
    text = session_text(session, ocr_limit=15)
    if not REFACTOR_RE.search(text):
        return None
    return RuleHit(
        dimension="practice",
        value="refactoring",
        confidence=0.7,
        explanation="refactor keyword visible",
    )


@rule(name="security_keywords", dim="practice")
def _security_keywords(session: dict):
    text = session_text(session, ocr_limit=15)
    if not SECURITY_RE.search(text):
        return None
    return RuleHit(
        dimension="practice",
        value="security_check",
        confidence=0.65,
        explanation="security-related keyword",
    )


@rule(name="linting_visible", dim="practice")
def _linting_visible(session: dict):
    text = session_text(session, ocr_limit=15)
    if not LINTING_RE.search(text):
        return None
    return RuleHit(
        dimension="practice",
        value="linting",
        confidence=0.8,
        explanation="linter mentioned/running",
    )


@rule(name="ci_visible", dim="practice")
def _ci_visible(session: dict):
    text = session_text(session, ocr_limit=15)
    if not CI_RE.search(text):
        return None
    return RuleHit(
        dimension="practice",
        value="ci_check",
        confidence=0.75,
        explanation="CI workflow visible",
    )
