## Pipeline context (standalone mode)

Stage 1 (rule classifier) and Stage 2 (embedding similarity) are both disabled.
- OBSERVED DIMENSIONS are unavailable.
- CANDIDATE TICKETS are keyword-prefiltered from all open tickets (no cosine scores).

Your job is extended — add a `dimensions` field to your JSON output:

  {"task_key": "KAN-86", "confidence": 0.75, "reasoning": "...",
   "dimensions": {"activity": ["coding"], "intent": ["implementation"], "tool": ["vscode"]}}

Dimensions schema:
- Keys: activity, intent, engagement, collaboration, tool, topic, practice
- Values: list of lowercase snake_case strings (e.g. "code_review", "deep_work", "github_pr")
- Omit a dimension if no value is evident from the session evidence
- Return "dimensions": {} if the session has no clear activity signals
- If task_key is null, still infer dimensions when the evidence supports them
