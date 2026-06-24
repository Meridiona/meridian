# Minimal-hour representation — deep research synthesis (2026-06-23)

Goal: compress ~1 hour of noisy screen-capture activity (40–60 app sessions, ~150k tok)
into a MINIMAL, information-preserving, noise-reduced representation a sub-4B on-device LLM
can summarise into "what I worked on this hour" → then bind to PM tickets / create-new.
Principle: **lose DATA, not INFORMATION, and lose NOISE.**

Research = 104-agent deep-research workflow, claims adversarially verified (3-0 unless noted).
Full report: tool-results/b5vd71fei.txt (run wf_256c2188-301).

## Theoretical grounding
"Lose data not information" = **information-bottleneck / rate-distortion**. BottleSum (West et al.
EMNLP 2019, arXiv:1909.07405) operationalises "compress X to best predict downstream Y". Nagle et al.
(NeurIPS 2024, arXiv:2407.15504) derive a computable distortion-rate LP = an optimal limit to measure against.

## Verified findings (each 3-0 unless noted)

1. **SemDeDup is the core method** (Meta, ICML 2023, arXiv:2303.09540): embed → k-means →
   intra-cluster cosine threshold → keep one representative. Removes ~50% with minimal loss.
   Directly targets our measured failure mode: 91% of OCR lines are LEXICALLY unique
   (per-frame OCR variance "Baseline"/"Daselıne") → MinHash/SimHash FAIL; need SEMANTIC dedup.

2. **Pure clustering drops rare signal islands — use FACILITY-LOCATION** (FLOC, ICLR 2026,
   openreview tPhcrP75OP): "clustering-based approaches fail to capture rare but important
   tokens... focus on densely populated regions." Maps to our data: dense recurring chrome =
   dense region (collapse); rare KAN-231/file-paths = sparse tail (preserve). Facility-location
   submodular selection covers head AND tail; greedy has (1−1/e)≈0.632 guarantee (Nemhauser 1978),
   lazy-greedy identical solution faster. Knapsack variant needed for heterogeneous token costs
   (0–30s micro-fragments vs 12k-char IDE captures).

3. **Budgeted set-level selection w/ redundancy penalty** (AdaGReS, arXiv:2512.25052 — MEDIUM
   conf, unreviewed Dec-2025 preprint): top-k returns redundant chunks; optimise relevance MINUS
   intra-set redundancy greedily under token budget. Method design corroborated by Lin & Bilmes,
   Das & Kempe; efficacy numbers self-reported.

4. **LLMLingua-2 = on-device extractive pruner** (Microsoft, ACL 2024, arXiv:2403.12968):
   prompt compression as per-token keep/discard classification, bidirectional XLM-RoBERTa-large
   (~560M), GPT-4-distilled, 3–6× faster than LLMLingua/LongLLMLingua/Selective-Context. Fits resident.

5. **Query-aware compression beats query-agnostic — our task is fixed** (LongLLMLingua,
   arXiv:2310.06839): question-conditioning can BEAT the full prompt (+21.4% at ~4× fewer tokens).
   Doc salience = perplexity(question|doc); token salience = contrastive perplexity = conditional PMI.
   Cost: per-window re-compression (~2×), acceptable since "what did I work on" is constant.

6. **Structured side-input beats raw flat text + preserves RELATIONS** (StrucSum, EACL 2026,
   arXiv:2505.22950): graph (sentences=nodes, cosine edges) + centrality injection; Centrality-Guided
   Masking cuts tokens 40–50%, structure injection +19.2 FactCC / +8.0 SummaC on ArXiv (2-1 on magnitude).
   = the mechanism for "feed features to improve summary": ticket↔file↔url entity graph + timeline ordering.

7. **TRAIN vs PROMPT → prompt-first** (MEDIUM). No verified benchmark shows fine-tuned sub-2B >
   prompted SLM on noisy-OCR activity logs. Hierarchical segment→condense→fine-tune architecture
   exists (arXiv:2410.06520) but on clean dialogue. Selection-p "10× at 0.8% drop" efficacy REFUTED 0-3
   (arXiv:2410.11786). Verdict: prompt-first; distill the 9B's summaries only after measuring a prompted ceiling.

## CROSS-CUTTING CAVEAT
EVERY benchmark is from an ADJACENT domain (web-scale dedup, VLM tokens, clean QA, clean ArXiv).
NONE validated on garbled OCR + UI boilerplate + micro-fragments. Mechanisms map cleanly; the NUMBERS
(50%, +21.4%, 40–50%) are transferable analogies, NOT measured on our data. Competitor internals
(Pieces/Recall/Rewind) came back essentially UNVERIFIED — only Pieces' public note (20-min windows +
hierarchical summaries, "just enough cleaning — over-cleaning increases hallucinations").

## Must-measure before building (open questions)
1. Does SemDeDup collapse OCR per-frame variants at the SPAN level on real session_text
   (chosen encoder + cosine threshold)? All cited validation is document/datapoint-level.
2. Does LLMLingua-2's keep/discard classifier PRESERVE or DESTROY signal islands (KAN-xxx,
   file paths) under OCR corruption it never saw in clean GPT-4-distilled training?

## RECOMMENDED PIPELINE ("minimal hour brief")
1. EXTRACT signal islands first (regex; tickets 48% / paths 58% / cmds 45% / domains 32% coverage measured)
2. SEMANTIC DEDUP spans (SemDeDup: small encoder → k-means → cosine threshold) ← kills semantic redundancy
3. FACILITY-LOCATION greedy select under token budget ← keeps sparse islands, collapses dense chrome
4. (optional) LLMLingua-2 query-aware prune on survivors
5. BUILD structured side-input: timeline (ordering=relations) + entity graph + app-transitions
6. → sub-4B SLM "hour brief" naming distinct task-threads
7. → reranker binds each thread → ticket / create-new (Stage B, 72%/10FB measured)

RAM: small encoder (~1–2GB) + LLMLingua-2 (560M) + sub-4B SLM, sequential → under the 7GB 9B.

## Key sources
- SemDeDup arXiv:2303.09540 · FLOC openreview tPhcrP75OP · AdaGReS arXiv:2512.25052
- LLMLingua-2 arXiv:2403.12968 · LongLLMLingua arXiv:2310.06839 · rate-distortion arXiv:2407.15504
- StrucSum arXiv:2505.22950 · hierarchical arXiv:2410.06520 · BottleSum arXiv:1909.07405
- Pieces hierarchical-summarization blog · RAPTOR arXiv:2401.18059
