"""Activity-dimension rules — the high-level "what is the user doing".

Single-value dimension. Resolution picks the highest-confidence hit, so
rules that have stronger evidence (e.g. a Cursor chat panel visible) should
return higher confidence than coarse signals (e.g. just "VS Code is open").
"""
from __future__ import annotations

import re

from agents.rules import RuleHit, rule, session_text, extract_urls

IDE_APPS = {"Code", "Cursor", "Xcode", "PyCharm", "IntelliJ IDEA",
            "WebStorm", "Sublime Text", "Zed", "Neovim", "vim", "nvim"}
TERMINAL_APPS = {"Terminal", "iTerm2", "Alacritty", "kitty", "WezTerm", "Warp", "Ghostty"}
COMMS_APPS = {"Slack", "Microsoft Teams", "Discord", "Mail", "Outlook",
              "Messages", "WhatsApp", "Telegram"}
MEETING_APPS = {"zoom.us", "Zoom", "Google Meet", "Microsoft Teams", "FaceTime",
                "Webex", "GoToMeeting"}
DESIGN_APPS = {"Figma", "Sketch", "Adobe Photoshop", "Adobe Illustrator",
               "Linear", "Whimsical", "Miro"}
NOTES_APPS = {"Notes", "Obsidian", "Notion", "Bear", "Drafts"}
AI_CHAT_APPS = {"Claude", "ChatGPT", "Cursor", "Ollama", "Gemini", "Perplexity"}
INSTALLER_APPS = {"Installer", "Software Update", "App Store", "SecurityAgent"}

LEARNING_HOSTS_RE = re.compile(
    r"\b(docs\.\w+|youtube\.com|developer\.\w+|stackoverflow\.com|"
    r"medium\.com|substack\.com|coursera\.org|udemy\.com|"
    r"freecodecamp\.org|tutorialspoint\.com|pluralsight\.com)\b",
    re.I,
)
DEPLOY_KEYWORDS_RE = re.compile(
    r"\b(kubectl|terraform|docker push|docker run|aws |gcloud |"
    r"helm |ansible|ssh \w+@|ssh-agent|systemctl)\b",
    re.I,
)
PR_REVIEW_RE = re.compile(
    r"github\.com/[^/\s]+/[^/\s]+/pull/\d+(/files)?",
    re.I,
)
DOCS_FILE_RE = re.compile(r"\b\S+\.(md|mdx|rst|adoc|txt)\b", re.I)
CODE_FILE_RE = re.compile(
    r"\b\S+\.(rs|py|ts|tsx|js|jsx|go|java|kt|swift|c|cpp|h|hpp|sql|sh|toml|yaml|yml|json|html|css|scss)\b",
    re.I,
)


def _app(session: dict) -> str:
    return session.get("app_name") or ""


@rule(name="ide_with_code_files", dim="activity")
def _ide_with_code_files(session: dict):
    if _app(session) not in IDE_APPS:
        return None
    text = session_text(session)
    if not CODE_FILE_RE.search(text):
        return None
    return RuleHit(
        dimension="activity",
        value="coding",
        confidence=0.9,
        explanation=f"{_app(session)} + code-file extension visible",
    )


@rule(name="cursor_with_chat_panel", dim="activity")
def _cursor_with_chat_panel(session: dict):
    if _app(session) != "Cursor":
        return None
    text = session_text(session).lower()
    has_chat = any(k in text for k in (
        "chat", "compose", "you are", "claude", "gpt", "model:", "tab to",
    ))
    if not has_chat:
        return None
    return [
        RuleHit(dimension="activity", value="ai_pair_programming", confidence=0.92,
                explanation="Cursor with chat panel visible"),
        RuleHit(dimension="collaboration", value="ai_assisted", confidence=0.95),
    ]


@rule(name="ai_chat_app", dim="activity")
def _ai_chat_app(session: dict):
    app = _app(session)
    if app not in AI_CHAT_APPS or app == "Cursor":  # Cursor handled separately
        return None
    text = session_text(session).lower()
    looks_like_prompts = any(k in text for k in (
        "prompt", "you are", "system prompt", "few-shot", "chain of thought",
        "json mode", "tool call", "function call",
    ))
    hits = [
        RuleHit(dimension="collaboration", value="ai_assisted", confidence=0.85),
    ]
    if looks_like_prompts:
        hits.append(RuleHit(
            dimension="activity",
            value="prompt_engineering",
            confidence=0.7,
            explanation=f"{app} with prompt-engineering vocabulary",
        ))
    else:
        hits.append(RuleHit(
            dimension="activity",
            value="ai_pair_programming",
            confidence=0.7,
            explanation=f"{app} chat session",
        ))
    return hits


@rule(name="github_pr_review", dim="activity")
def _github_pr_review(session: dict):
    if not PR_REVIEW_RE.search(session_text(session)):
        return None
    return [
        RuleHit(dimension="activity", value="code_review", confidence=0.9,
                explanation="GitHub PR/files page open"),
        RuleHit(dimension="practice", value="code_review_done", confidence=0.85),
    ]


@rule(name="docs_browsing", dim="activity")
def _docs_browsing(session: dict):
    text = session_text(session)
    if not LEARNING_HOSTS_RE.search(text):
        return None
    return RuleHit(
        dimension="activity",
        value="learning",
        confidence=0.75,
        explanation="docs/learning host in URLs",
    )


@rule(name="comms_app", dim="activity")
def _comms_app(session: dict):
    if _app(session) not in COMMS_APPS:
        return None
    return RuleHit(
        dimension="activity",
        value="communication",
        confidence=0.9,
        explanation=f"{_app(session)} (comms app)",
    )


@rule(name="meeting_app", dim="activity")
def _meeting_app(session: dict):
    app = _app(session)
    if app in MEETING_APPS or "zoom" in app.lower() or "meet" in app.lower():
        return [
            RuleHit(dimension="activity", value="meeting", confidence=0.95,
                    explanation=f"{app}"),
            RuleHit(dimension="collaboration", value="team_review", confidence=0.7),
        ]
    return None


@rule(name="design_app", dim="activity")
def _design_app(session: dict):
    if _app(session) not in DESIGN_APPS:
        return None
    return RuleHit(
        dimension="activity",
        value="design",
        confidence=0.9,
        explanation=f"{_app(session)}",
    )


@rule(name="notes_app", dim="activity")
def _notes_app(session: dict):
    if _app(session) not in NOTES_APPS:
        return None
    return RuleHit(
        dimension="activity",
        value="planning",
        confidence=0.6,
        explanation=f"{_app(session)} (notes/planning)",
    )


@rule(name="terminal_with_deploy_keywords", dim="activity")
def _terminal_with_deploy(session: dict):
    if _app(session) not in TERMINAL_APPS:
        return None
    text = session_text(session)
    if not DEPLOY_KEYWORDS_RE.search(text):
        return None
    return RuleHit(
        dimension="activity",
        value="deployment_devops",
        confidence=0.85,
        explanation="Terminal with deploy/devops command",
    )


@rule(name="installer_or_security", dim="activity")
def _installer_or_security(session: dict):
    if _app(session) not in INSTALLER_APPS:
        return None
    return RuleHit(
        dimension="activity",
        value="admin",
        confidence=0.9,
        explanation=f"{_app(session)}",
    )


@rule(name="docs_editing", dim="activity")
def _docs_editing(session: dict):
    """Editing markdown / rst files in an IDE → documentation activity."""
    if _app(session) not in IDE_APPS:
        return None
    text = session_text(session)
    if not DOCS_FILE_RE.search(text):
        return None
    return [
        RuleHit(dimension="activity", value="documentation", confidence=0.7,
                explanation="IDE editing .md/.rst/.adoc"),
        RuleHit(dimension="practice", value="documentation_updated", confidence=0.7),
    ]


@rule(name="ide_no_signal", dim="activity")
def _ide_no_signal(session: dict):
    """IDE open but no code/doc/chat signal — coarse fallback."""
    if _app(session) not in IDE_APPS:
        return None
    text = session_text(session)
    if CODE_FILE_RE.search(text) or DOCS_FILE_RE.search(text):
        return None
    return RuleHit(
        dimension="activity",
        value="coding",
        confidence=0.4,
        explanation="IDE open, no specific file signal",
    )
