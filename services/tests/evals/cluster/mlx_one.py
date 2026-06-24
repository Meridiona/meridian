"""Run ONE MLX model on a prompt file, print JSON {summary, peak_gb, latency}.
Runs as a subprocess so the model fully unloads on exit. Thinking disabled.
Usage: python mlx_one.py <repo> <prompt_file> <temp> <top_p> <top_k>
"""
import sys, json, time
import mlx.core as mx
from mlx_lm import load, generate
from mlx_lm.sample_utils import make_sampler

repo, pf = sys.argv[1], sys.argv[2]
temp, top_p, top_k = float(sys.argv[3]), float(sys.argv[4]), int(sys.argv[5])
prompt = open(pf).read()

t0 = time.time()
model, tok = load(repo)
load_s = time.time() - t0

msgs = [{"role": "user", "content": prompt}]
try:
    text_in = tok.apply_chat_template(msgs, add_generation_prompt=True, tokenize=False,
                                      enable_thinking=False)
except TypeError:
    text_in = tok.apply_chat_template(msgs, add_generation_prompt=True, tokenize=False)

sampler = make_sampler(temp=temp, top_p=top_p, top_k=top_k)
t1 = time.time()
out = generate(model, tok, prompt=text_in, max_tokens=400, sampler=sampler, verbose=False)
gen_s = time.time() - t1
peak = mx.get_peak_memory() / 1e9

print("@@JSON@@" + json.dumps({
    "summary": out.strip(), "peak_gb": round(peak, 2),
    "load_s": round(load_s, 1), "gen_s": round(gen_s, 1),
}))
