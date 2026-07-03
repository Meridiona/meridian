"""HTTP clients for worklog + ticket generation — the agno-free finalize path.

Mirrors ``classifier._post_classify``: thin urllib POSTs to the MLX server's
``/generate_worklog`` and ``/propose_ticket`` endpoints, returning the pipeline's
typed models.

Failure posture (deliberate):
  • A TRANSPORT/HTTP failure (connection reset, timeout, the endpoint's 500 on an
    inference error) is RAISED, not swallowed — so the agno Step retries and, if it
    still fails, the hour fails and the Rust driver re-runs the whole hour next pass.
    A transient model blip must never silently drop a matched ticket's worklog.
  • A well-formed but empty/unusable result (no summary; an abstention; unparseable
    JSON the server already absorbed into empty fields) returns ``None`` — a
    deterministic "no draft", not worth retrying.
"""
from __future__ import annotations

import json
import logging
import urllib.request

from agents.worklog_pipeline.classifier import TaskBinding
from agents.worklog_pipeline.models import ProposedTicket, WorklogDraft

log = logging.getLogger("meridian.worklog.generation")


def _post(server_url: str, path: str, body: dict, timeout: float = 300) -> dict:
    """POST JSON and return the parsed response. Raises on any transport/HTTP/
    decode error so the caller's retry tier (agno Step → Rust driver) re-runs."""
    req = urllib.request.Request(
        f"{server_url}{path}",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def generate_worklog(
    server_url: str,
    *,
    report: str,
    body: str,
    task_key: str,
    title: str,
    description: str,
    why: str,
    is_new: bool = False,
    traceparent: str | None = None,
) -> WorklogDraft | None:
    """POST /generate_worklog for one ticket.

    Returns ``None`` (no draft persisted) when the model produced no usable
    summary or the response can't be validated. Raises on transport/HTTP failure.
    """
    resp = _post(server_url, "/generate_worklog", {
        "report": report, "distilled_body": body, "task_key": task_key,
        "title": title, "description": description, "why": why, "is_new": is_new,
        "traceparent": traceparent,
    })
    summary = (resp.get("summary") or "").strip()
    if not summary:
        log.warning("generation: worklog for %s came back empty — no draft", task_key)
        return None
    try:
        return WorklogDraft(
            summary=summary,
            what_shipped=resp.get("what_shipped", []),
            decisions=resp.get("decisions", []),
            confidence=resp.get("confidence", 0.0),
        )
    except (TypeError, ValueError) as exc:
        log.warning("generation: worklog parse failed for %s (%s)", task_key, exc)
        return None


def propose_ticket(
    server_url: str,
    *,
    report: str,
    body: str,
    matched: list[TaskBinding],
    titles: dict[str, str] | None = None,
    traceparent: str | None = None,
) -> ProposedTicket | None:
    """POST /propose_ticket.

    ``titles`` maps task_key → title so the "already matched" list the proposer
    sees names each ticket (not just its key). Returns the drafted ticket, or
    ``None`` when the model abstains (residual work not worth a ticket) or returned
    an unusable draft (no title). Raises on transport/HTTP failure so a transient
    blip retries rather than abstaining.
    """
    titles = titles or {}
    resp = _post(server_url, "/propose_ticket", {
        "report": report, "distilled_body": body,
        "matched": [{"task_key": b.task_key, "title": titles.get(b.task_key, ""), "why": b.why}
                    for b in matched],
        "traceparent": traceparent,
    })
    if not resp.get("should_propose"):
        log.info("generation: propose abstained (%s)", (resp.get("reasoning") or "")[:120])
        return None
    title = (resp.get("title") or "").strip()
    if not title:
        log.info("generation: propose said yes but gave no title — dropping")
        return None
    try:
        return ProposedTicket(
            should_propose=True,
            issue_type="Bug" if str(resp.get("issue_type", "Task")).lower() == "bug" else "Task",
            title=title,
            description=(resp.get("description") or "").strip(),
            reasoning=(resp.get("reasoning") or "").strip(),
        )
    except (TypeError, ValueError) as exc:
        log.warning("generation: propose parse failed (%s)", exc)
        return None
