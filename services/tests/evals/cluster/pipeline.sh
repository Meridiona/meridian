#!/bin/bash
cd /Users/adityaharish/Documents/Meridiona/meridian/services/tests/evals/cluster
V=/Users/adityaharish/Documents/Meridiona/meridian/services/.venv
export TOKENIZERS_PARALLELISM=false HF_HUB_DISABLE_PROGRESS_BARS=1 PYTORCH_ENABLE_MPS_FALLBACK=1
# wait for any running qwen3 embed to finish
while pgrep -f "run_cluster.py --days 10 --models qwen3-0.6b" >/dev/null; do sleep 10; done
echo "### qwen3 phase done; embedding jina ###"
"$V/bin/python" run_cluster.py --days 10 --models jina-v3 --strip yes > jina.log 2>&1
echo "### jina done; final combined comparison (all cached) ###"
"$V/bin/python" run_cluster.py --days 10 --models bge-small,qwen3-0.6b,jina-v3 --strip yes --dump > final.log 2>&1
echo "### ALL DONE ###"
