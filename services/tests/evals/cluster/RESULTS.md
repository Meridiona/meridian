# Session clustering — offline feasibility study (Stage A)

**Question:** can we group `app_sessions` by `session_text` + time into "same unit of work"
buckets (so a downstream stage binds a whole bucket to a ticket / spawns a new task ONCE),
instead of running the 9B per-session classifier? Goal: better accuracy, less RAM.

**Verdict: FEASIBLE, low regression risk.** Conservative clustering produces task-pure,
coherent buckets and is *more self-consistent* than the per-session 9B.

## Data
- Last 10 days, in-scope = all apps EXCEPT self-summarising CLI agents (Claude Code/Codex/Copilot).
- Noise gate (dur >= 15s, text > 50 chars): **1,607 usable sessions** (~75% are <15s micro-fragments, dropped).
- 764 have a 9B `task_key` (used as a NOISY reference label, NOT ground truth).

## Method
clean text -> embed (vector) -> affinity = cosine x time-decay kernel -> agglomerative
(avg linkage, distance threshold = the conservativeness dial). No reranker, no PM tasks.

## Key findings
1. **Time-gating is load-bearing.** Pure text similarity collapses everything into 1–9
   mega-clusters (boilerplate/OCR uniformity). With an `exp(-dt/tau)` gate it works.
2. **qwen3-0.6b > bge-small.** At equal precision, qwen3 captures ~2x more same-task pairs.
   - qwen3 tau=120/thr=0.3: PREC 0.79, REC 0.31, 247 clusters, 95% grouped, homogeneity 0.86
   - bge   tau=120/thr=0.3: PREC 0.80, REC 0.14
3. **Measured precision (~0.75–0.82) is a LOWER BOUND.** The "impure" clusters are mostly
   correct: the 9B sprays 2–3 ticket labels across one obviously-continuous burst (same file,
   same 20-min window). Cluster-level dominance = **0.80–0.87**; only **12–23 genuinely-mixed
   clusters in 10 days**. Clustering is MORE consistent than the per-session 9B.
4. **Untracked work clusters cleanly** (HN post-writing, F1 videos=idle, finance portal) —
   exactly the work the current `task_key IS NOT NULL` worklog filter silently drops.
5. **Boilerplate stripping via exact n-grams is ineffective** on garbled OCR (0% removed on
   Chrome — each capture garbles differently). The time-gate already neutralises cross-context
   false merges, so stripping is not needed (strip Y ≈ strip N).
6. **Hybrid (dense + TF-IDF lexical) is only marginal** on this data (PR-frontier): light mix
   (w=0.15) nudges precision ~+0.02 at the conservative end, equal/worse elsewhere. Dense wins on simplicity.
7. **jina-v3** not evaluated — broken custom-code import (`mha.py`); skipped (qwen3 wins and
   pairs with the existing Qwen3-Reranker).

## Recommendation
- Embedder: **Qwen3-Embedding-0.6B** (MLX), ~1.2 GB. Replaces the 7 GB 9B for grouping.
- Affinity: dense cosine x time gate (tau ~60–120 min). Optional light TF-IDF mix (w~0.15).
- Clustering: agglomerative avg-linkage, distance threshold ~0.3–0.4 (conservative end:
  high purity, singletons fall back to per-session = today's behaviour, so no regression).

## Open / next
- **Reranker refinement** (approved fallback): use Qwen3-Reranker-0.6B to split the ~15–20
  genuinely-mixed clusters / confirm borderline merges (precision push).
- **Stage B** (deferred): bucket -> ticket via embed+rerank+abstain, or spawn new task.
- Cleaner ground truth: 9B labels are noisy; hand-label ~200 sessions for a real precision number.

## Files
`clib.py` (data/clean/affinity/cluster/eval) · `embed.py` (cached multi-model) ·
`run_cluster.py` (sweep driver) · `cache/*.npy` (embeddings). Re-run:
`python run_cluster.py --days 10 --models qwen3-0.6b --strip yes --dump`
