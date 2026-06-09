"""Dynamic local LLM selector for Apple Silicon Macs.

Priority order for every inference call:
  1. Reuse any already-running LLM server with a model loaded in memory
     (Ollama /api/ps, LM Studio state==loaded, llama.cpp /props, mlx_lm, Apple FM)
  2. Apple Intelligence — zero extra RAM, instant, macOS 26+ only
  3. Load the best-fitting MLX model within the available Metal budget × budget_pct

Metal headroom (mx.metal.device_info) is the primary memory signal — it operates
at the same layer where allocations actually succeed or fail, unlike vm_stat which
operates at the OS virtual-memory layer.

Thermal pressure (libnotify com.apple.system.thermalpressurelevel) caps model
selection when the machine is throttling, regardless of available headroom.

Chip specs use the lower GPU-bin value where two bins share the same brand string
(e.g. M3 Max 30-core = 300 GB/s vs 40-core = 400 GB/s — we use 300 as the
conservative estimate). M4 Ultra is omitted — it was cancelled; Mac Pro was
discontinued March 2026 without an M4 Ultra update. M5 Ultra is omitted — not
yet shipped as of mid-2026.
"""
# meridian — normalises screenpipe activity into structured app sessions
from __future__ import annotations

import ctypes
import json
import logging
import os
import platform
import re
import socket
import subprocess
import sys
import time
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

log = logging.getLogger("agents.llm_selector")

from opentelemetry import trace as trace_api
_tracer = trace_api.get_tracer(__name__)

# ── Chip specs: (gpu_cores_lower_bin, mem_bw_gbs_lower_bin) ──────────────────
# 18 entries — every Apple Silicon Mac chip shipped as of mid-2026.
# Where two GPU/BW tiers share the same brand string, the lower-bandwidth bin
# is used as a conservative estimate so model selection errs toward smaller.
_CHIP_SPECS: dict[str, tuple[int, int]] = {
    "apple m1":       (7,   68),
    "apple m1 pro":   (14,  200),
    "apple m1 max":   (24,  400),
    "apple m1 ultra": (48,  800),
    "apple m2":       (8,   100),
    "apple m2 pro":   (16,  200),
    "apple m2 max":   (30,  400),
    "apple m2 ultra": (60,  800),
    "apple m3":       (10,  100),
    "apple m3 pro":   (14,  150),  # bandwidth regression vs M2 Pro (192-bit bus)
    "apple m3 max":   (30,  300),  # 30-core bin=300 GB/s; 40-core bin=400 GB/s
    "apple m3 ultra": (60,  800),
    "apple m4":       (10,  120),
    "apple m4 pro":   (20,  273),
    "apple m4 max":   (32,  410),  # 32-core bin=410 GB/s; 40-core bin=546 GB/s
    "apple m5":       (8,   154),  # 8-core bin (MacBook Air base); 10-core also exists
    "apple m5 pro":   (16,  307),  # 16-core or 20-core bin
    "apple m5 max":   (32,  460),  # 32-core bin=460 GB/s; 40-core bin=614 GB/s
}

# ── MLX model catalog ─────────────────────────────────────────────────────────
# (id, backend, min_ram_gb, quality_score, hf_id)
# Ordered largest → smallest so select() returns the best that fits.
_MODELS = [
    ("llama3.3-70b",    "mlx", 40.0, 92, "mlx-community/Llama-3.3-70B-Instruct-4bit"),
    ("r1-70b",          "mlx", 40.0, 90, "mlx-community/DeepSeek-R1-Distill-Llama-70B-4bit"),
    ("qwen3.6-35b-moe", "mlx", 21.0, 88, "mlx-community/Qwen3.6-35B-A3B-4bit"),
    ("r1-32b",          "mlx", 19.0, 85, "mlx-community/DeepSeek-R1-Distill-Qwen-32B-4bit"),
    ("phi-4",           "mlx",  8.5, 80, "mlx-community/phi-4-4bit"),
    ("r1-14b",          "mlx",  8.5, 78, "mlx-community/DeepSeek-R1-Distill-Qwen-14B-4bit"),
    ("gemma3-12b",      "mlx",  7.0, 75, "mlx-community/gemma-3-12b-it-qat-4bit"),
    ("qwen3.5-9b-optiq","mlx",  6.5, 74, "mlx-community/Qwen3.5-9B-OptiQ-4bit"),  # eval-tuned classifier default
    ("qwen3.5-4b",      "mlx",  2.5, 65, "mlx-community/Qwen3.5-4B-MLX-4bit"),
    ("llama3.2-3b",     "mlx",  1.8, 62, "mlx-community/Llama-3.2-3B-Instruct-4bit"),
    ("apple-intelligence", "apple_fm", 0.0, 60, None),
]


# ─────────────────────────── Running server discovery ────────────────────────

@dataclass
class RunningServer:
    runtime: str           # "ollama" | "lmstudio" | "llamacpp" | "mlxlm" | "apple_fm"
    base_url: str          # OpenAI-compatible base URL; empty string for apple_fm
    loaded_models: list[str]
    best_model: str        # first loaded model, for convenience


def _tcp_open(host: str, port: int, timeout: float = 0.5) -> bool:
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def _get_json(url: str, headers: dict | None = None,
              timeout: float = 2.0) -> tuple[dict | None, int | None]:
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read()), r.status
    except urllib.error.HTTPError as e:
        return None, e.code
    except Exception:
        return None, None


def discover_running_servers() -> list[RunningServer]:
    """Probe known local LLM server ports; return servers with a model in memory.

    Probe order: Ollama (11434) → LM Studio (1234) → port 8080
    (llama.cpp disambiguated from mlx_lm via /props) → Apple FoundationModels.
    Total latency: <200 ms on a healthy system (0.5 s TCP timeout per port).
    """
    with _tracer.start_as_current_span("llm_selector.discover_servers") as span:
        try:
            found: list[RunningServer] = []

            # 1. Ollama — unique port; prefer /api/ps (loaded in VRAM), fall
            #    back to /api/tags (installed but unloaded — Ollama loads on demand).
            if _tcp_open("127.0.0.1", 11434):
                ver, _ = _get_json("http://127.0.0.1:11434/api/version")
                if ver and "version" in ver:
                    ps, _ = _get_json("http://127.0.0.1:11434/api/ps")
                    models = [m["name"] for m in (ps or {}).get("models", [])]
                    if models:
                        log.info("llm_selector: Ollama running loaded=%s", models)
                        span.add_event("ollama_found", {"models": str(models), "source": "ps"})
                    else:
                        # No model in VRAM — check installed models; Ollama will load on demand.
                        tags, _ = _get_json("http://127.0.0.1:11434/api/tags")
                        models = [m["name"] for m in (tags or {}).get("models", [])]
                        if models:
                            log.info("llm_selector: Ollama running installed=%s (will load on demand)", models)
                            span.add_event("ollama_found", {"models": str(models), "source": "tags"})
                        else:
                            log.debug("llm_selector: Ollama on :11434 — no models installed")
                    if models:
                        found.append(RunningServer(
                            "ollama", "http://127.0.0.1:11434/v1", models, models[0]))
                else:
                    log.debug("llm_selector: port 11434 open but not Ollama (no version endpoint)")
            else:
                log.debug("llm_selector: Ollama not running (port 11434 closed)")

            # 2. LM Studio — use native /api/v0/models (has state field) to distinguish
            #    in-memory models from installed-only ones. /v1/models omits state entirely.
            if _tcp_open("127.0.0.1", 1234):
                native, native_status = _get_json("http://127.0.0.1:1234/api/v0/models")
                if native_status == 200 and native:
                    models = [m["id"] for m in native.get("data", [])
                              if m.get("state") == "loaded"]
                    log.debug("llm_selector: LM Studio /api/v0/models loaded=%s", models)
                else:
                    # Older LM Studio — fall back to /v1/models, take all listed models
                    data, status = _get_json("http://127.0.0.1:1234/v1/models")
                    models = [m["id"] for m in (data or {}).get("data", [])] if status == 200 else []
                    log.debug("llm_selector: LM Studio /v1/models fallback models=%s", models)
                if models:
                    found.append(RunningServer(
                        "lmstudio", "http://127.0.0.1:1234/v1", models, models[0]))
                    log.info("llm_selector: LM Studio running loaded=%s", models)
                    span.add_event("lmstudio_found", {"models": str(models)})
                else:
                    log.debug("llm_selector: LM Studio on :1234 — no models loaded")
            else:
                log.debug("llm_selector: LM Studio not running (port 1234 closed)")

            # 3. Port 8080 — llama.cpp or mlx_lm; /props 200 = llama.cpp, 404 = mlx_lm
            if _tcp_open("127.0.0.1", 8080):
                props, props_status = _get_json("http://127.0.0.1:8080/props")
                if props_status == 200 and props:
                    data, _ = _get_json("http://127.0.0.1:8080/v1/models")
                    models = [m["id"] for m in (data or {}).get("data", [])]
                    if models:
                        found.append(RunningServer(
                            "llamacpp", "http://127.0.0.1:8080/v1", models, models[0]))
                        log.info("llm_selector: llama.cpp running loaded=%s", models)
                        span.add_event("llamacpp_found", {"models": str(models)})
                    else:
                        log.debug("llm_selector: llama.cpp on :8080 — no models loaded")
                else:
                    data, status = _get_json("http://127.0.0.1:8080/v1/models")
                    if status == 200 and data:
                        models = [m["id"] for m in data.get("data", [])]
                        if models:
                            found.append(RunningServer(
                                "mlxlm", "http://127.0.0.1:8080/v1", models, models[0]))
                            log.info("llm_selector: mlx_lm on :8080 running loaded=%s", models)
                            span.add_event("mlxlm_found", {"models": str(models)})
                        else:
                            log.debug("llm_selector: mlx_lm on :8080 — no models loaded")
            else:
                log.debug("llm_selector: no server on port 8080")

            # 4. Apple FoundationModels — in-process, no port, macOS 26+.
            #    is_available() is an INSTANCE method returning (bool, reason).
            #    Catch broadly (not just ImportError): an absent OR API-skewed
            #    apple_fm_sdk must degrade to "not found", never crash the whole
            #    discovery sweep (which would push every caller to cloud).
            try:
                from apple_fm_sdk import SystemLanguageModel  # type: ignore[import]
                available, reason = SystemLanguageModel().is_available()
                if available:
                    found.append(RunningServer(
                        "apple_fm", "", ["apple-intelligence"], "apple-intelligence"))
                    log.info("llm_selector: Apple Intelligence available")
                    span.add_event("apple_fm_found")
                else:
                    log.debug("llm_selector: Apple Intelligence unavailable: %s", reason)
            except Exception as exc:  # noqa: BLE001 — SDK absent or version-skewed
                log.debug("llm_selector: Apple FM probe skipped: %s", exc)

            span.set_attribute("servers.found", len(found))
            span.set_attribute("servers.names", str([s.runtime for s in found]))
            return found
        except Exception as exc:
            span.record_exception(exc)
            raise


def _infer_via_server(server: RunningServer, system_prompt: str,
                      user_message: str, max_tokens: int) -> Optional[str]:
    if server.runtime == "apple_fm":
        return _infer_apple_intelligence(system_prompt, user_message, max_tokens)
    try:
        from openai import OpenAI  # type: ignore[import]
        client = OpenAI(base_url=server.base_url, api_key="local")
        resp = client.chat.completions.create(
            model=server.best_model,
            messages=[
                {"role": "system", "content": system_prompt},
                {"role": "user",   "content": user_message},
            ],
            max_tokens=max_tokens,
        )
        return resp.choices[0].message.content
    except Exception as exc:
        log.warning("llm_selector: %s inference failed: %s", server.runtime, exc)
        return None


# ─────────────────────────── Real-time compute probe ─────────────────────────

@dataclass
class ComputeSnapshot:
    metal_headroom_gb: float  # Metal budget minus active+cached MLX allocations
    thermal_level: int        # 0=nominal 1=moderate 2=heavy 3=trapping 4=sleeping
    cpu_pct: float
    screen_locked: bool
    chip_name: str
    mem_bw_gbs: int


# Sentinel returned by select_mlx_model_id() when Apple Intelligence is chosen.
APPLE_INTELLIGENCE_ID = "apple-intelligence"


def _apple_intelligence_available() -> bool:
    """Return True only when Apple Intelligence is genuinely usable.

    Checks three things in order (fail-fast):
      1. macOS 26+ (the minimum for the Foundation Models API)
      2. apple_fm_sdk is importable (installed in the venv)
      3. SystemLanguageModel().is_available() — the on-device model is downloaded
         and Apple Intelligence is enabled in System Settings

    Any failure → False (graceful degradation to smaller MLX or cloud).
    """
    try:
        macos_major = int(platform.mac_ver()[0].split(".")[0] or "0")
        if macos_major < 26:
            return False
        from apple_fm_sdk import SystemLanguageModel  # type: ignore[import]
        available, reason = SystemLanguageModel().is_available()
        if not available:
            log.debug("llm_selector: Apple Intelligence unavailable: %s", reason)
        return bool(available)
    except Exception as exc:  # noqa: BLE001
        log.debug("llm_selector: Apple Intelligence probe skipped: %s", exc)
        return False


def _metal_headroom_gb() -> tuple[float, str]:
    """Primary memory signal — headroom within Metal's recommended working set.

    Returns (headroom_gb, source) where source is 'mlx' or 'vm_stat'.
    """
    try:
        import mlx.core as mx  # type: ignore[import]
        info    = mx.device_info()
        ceiling = info["max_recommended_working_set_size"]
        active  = mx.get_active_memory()
        cached  = mx.get_cache_memory()
        gb = (ceiling - active - cached) / (1 << 30)
        log.debug(
            "llm_selector: metal headroom=%.1f GB (ceiling=%.1f active=%.1f cached=%.1f) source=mlx",
            gb, ceiling / (1 << 30), active / (1 << 30), cached / (1 << 30),
        )
        return gb, "mlx"
    except Exception:
        pass
    # Fallback: vm_stat free+inactive (less accurate, always available)
    try:
        page_size = int(subprocess.check_output(
            ["sysctl", "-n", "hw.pagesize"], timeout=2).strip())
        out = subprocess.check_output(["vm_stat"], timeout=2).decode()
        def pg(label: str) -> int:
            m = re.search(rf"{label}:\s+(\d+)", out)
            return int(m.group(1)) if m else 0
        gb = (pg("Pages free") + pg("Pages inactive")) * page_size / (1 << 30)
        log.debug("llm_selector: metal headroom=%.1f GB source=vm_stat (mlx unavailable)", gb)
        return gb, "vm_stat"
    except Exception:
        return 0.0, "unknown"


def _thermal_level() -> int:
    """Read macOS thermal pressure level without root via libnotify."""
    try:
        lib = ctypes.cdll.LoadLibrary("/usr/lib/libnotify.dylib")
        key = b"com.apple.system.thermalpressurelevel"
        tok = ctypes.c_int(0)
        st  = ctypes.c_uint64(0)
        lib.notify_register_check(key, ctypes.byref(tok))
        lib.notify_get_state(tok, ctypes.byref(st))
        lib.notify_cancel(tok)
        return int(st.value)
    except Exception:
        return 0


def _screen_locked() -> bool:
    try:
        import Quartz  # type: ignore[import]
        sess = Quartz.CGSessionCopyCurrentDictionary()
        return bool(sess and sess.get("CGSSessionScreenIsLocked", 0))
    except Exception:
        return False


def _sysctl(key: str) -> Optional[str]:
    try:
        r = subprocess.run(["sysctl", "-n", key],
                           capture_output=True, text=True, timeout=2)
        return r.stdout.strip() if r.returncode == 0 else None
    except Exception:
        return None


def probe_compute() -> ComputeSnapshot:
    with _tracer.start_as_current_span("llm_selector.probe_compute") as span:
        try:
            import psutil  # type: ignore[import]
            brand = _sysctl("machdep.cpu.brand_string") or ""
            key   = re.sub(r"\s+", " ", brand).lower()
            _, mem_bw = _CHIP_SPECS.get(key, (None, 0))
            headroom_gb, headroom_source = _metal_headroom_gb()
            thermal = _thermal_level()
            cpu_pct = psutil.cpu_percent(interval=0.5)
            locked  = _screen_locked()
            snap = ComputeSnapshot(
                metal_headroom_gb=headroom_gb,
                thermal_level=thermal,
                cpu_pct=cpu_pct,
                screen_locked=locked,
                chip_name=brand,
                mem_bw_gbs=mem_bw or 0,
            )
            span.set_attribute("compute.headroom_gb",    round(headroom_gb, 2))
            span.set_attribute("compute.headroom_source", headroom_source)
            span.set_attribute("compute.thermal_level",  thermal)
            span.set_attribute("compute.thermal_ok",     thermal < 2)
            span.set_attribute("compute.cpu_pct",        round(cpu_pct, 1))
            span.set_attribute("compute.screen_locked",  locked)
            span.set_attribute("compute.chip",           brand)
            span.set_attribute("compute.mem_bw_gbs",     mem_bw or 0)
            log.info(
                "llm_selector: compute headroom=%.1f GB thermal=%d cpu=%.0f%% "
                "locked=%s chip=%s mem_bw=%d GB/s source=%s",
                headroom_gb, thermal, cpu_pct, locked, brand, mem_bw or 0, headroom_source,
            )
            return snap
        except Exception as exc:
            span.record_exception(exc)
            raise


# ─────────────────────────── Model selection ─────────────────────────────────

def _select_mlx_entry(headroom_gb: float, budget_pct: float,
                      thermal_level: int, apple_intelligence: bool) -> Optional[tuple]:
    budget = headroom_gb * budget_pct
    # Under heavy throttle (level ≥ 2) cap to medium tier — don't add heat
    if thermal_level >= 2:
        budget = min(budget, 9.0)
    for entry in _MODELS:
        model_id, backend, min_ram, _, _ = entry
        if backend == "apple_fm" and not apple_intelligence:
            continue
        if min_ram <= budget:
            return entry
    return None


# ─────────────────────────── Cached MLX model ────────────────────────────────

_mlx_cache: dict[str, tuple] = {}


# ─────────────────────────── Public entry point ──────────────────────────────

def local_infer(system_prompt: str, user_message: str,
                budget_pct: float = 0.5,
                max_tokens: int = 1024) -> Optional[str]:
    """Run inference on the best available local model.

    Returns the model's text response, or None if nothing is available
    (caller falls back to the cloud path).

    Priority:
      1. Already-running server with a model in memory (zero load cost)
      2. Apple Intelligence (macOS 26+, zero extra RAM)
      3. Load best MLX model within Metal headroom × budget_pct
    """
    if platform.system() != "Darwin":
        return None
    brand = _sysctl("machdep.cpu.brand_string") or ""
    if not brand.startswith("Apple M"):
        return None

    # Step 1: reuse any already-running server
    for server in discover_running_servers():
        result = _infer_via_server(server, system_prompt, user_message, max_tokens)
        if result is not None:
            return result

    # Step 2: nothing running — probe compute and load the best model
    try:
        snap = probe_compute()
    except Exception as exc:
        log.warning("llm_selector: compute probe failed: %s", exc)
        return None

    log.info(
        "llm_selector: headroom=%.1f GB thermal=%d cpu=%.0f%% locked=%s chip=%s",
        snap.metal_headroom_gb, snap.thermal_level,
        snap.cpu_pct, snap.screen_locked, snap.chip_name,
    )

    # Relax budget when screen is locked — user won't feel the latency
    effective_pct = min(0.8, budget_pct * 1.5) if snap.screen_locked else budget_pct

    apple_intelligence = _apple_intelligence_available()

    entry = _select_mlx_entry(snap.metal_headroom_gb, effective_pct,
                              snap.thermal_level, apple_intelligence)
    if entry is None:
        log.info("llm_selector: no model fits %.1f GB budget", snap.metal_headroom_gb * effective_pct)
        return None

    _, backend, _, _, hf_id = entry
    if backend == "apple_fm":
        return _infer_apple_intelligence(system_prompt, user_message, max_tokens)
    return _infer_mlx(hf_id, system_prompt, user_message, max_tokens)


def _infer_apple_intelligence(system: str, user: str, max_tokens: int) -> Optional[str]:
    try:
        import asyncio
        from apple_fm_sdk import LanguageModelSession  # type: ignore[import]
        async def _run() -> str:
            # Constructor kwarg is `instructions` (the system prompt). respond()
            # is a coroutine; in free-form mode it resolves to a plain str, while
            # guided modes return a GeneratedContent with a .content attr — handle
            # both. (max_tokens is governed by the SDK's own GenerationOptions.)
            session = LanguageModelSession(instructions=system)
            r = await session.respond(user)
            return getattr(r, "content", r)
        return asyncio.run(_run())
    except Exception as exc:
        log.warning("llm_selector: apple_fm failed: %s", exc)
        return None


def _infer_mlx(model_id: str, system: str, user: str, max_tokens: int) -> Optional[str]:
    try:
        os.environ.setdefault("MLX_NUM_THREADS", "4")  # leave P-cores for foreground
        from mlx_lm import load, generate  # type: ignore[import]
        if model_id not in _mlx_cache:
            log.info("llm_selector: loading %s (first call this process)", model_id)
            _mlx_cache[model_id] = load(model_id)
        model, tokenizer = _mlx_cache[model_id]
        prompt = f"<|system|>\n{system}\n<|user|>\n{user}\n<|assistant|>\n"
        return generate(model, tokenizer, prompt=prompt,
                        max_tokens=max_tokens, verbose=False)
    except Exception as exc:
        log.warning("llm_selector: mlx_lm failed: %s", exc)
        return None


def _hf_model_cached(hf_id: "str | None") -> bool:
    """True when a HuggingFace repo's weights are already in the local cache.

    A best-effort filesystem check so dynamic selection never picks a catalog
    model that would trigger a multi-GB ``snapshot_download`` (online) or raise
    (offline) inside ``mlx_lm.load`` at server startup. Honours HF_HUB_CACHE /
    HF_HOME, falling back to ~/.cache/huggingface/hub. Mirrors the
    "best among what's present" philosophy of discover_running_servers().
    """
    if not hf_id:
        return False
    hf_home = os.environ.get("HF_HOME")
    cache = (
        os.environ.get("HF_HUB_CACHE")
        or (os.path.join(hf_home, "hub") if hf_home else None)
        or str(Path.home() / ".cache" / "huggingface" / "hub")
    )
    snapshots = Path(cache) / ("models--" + hf_id.replace("/", "--")) / "snapshots"
    if not snapshots.is_dir():
        return False
    # A revision directory that actually contains files (not just an empty shell).
    for rev in snapshots.iterdir():
        if rev.is_dir() and any(rev.iterdir()):
            return True
    return False


def select_mlx_model_id(
    preferred_hf_id: "str | None" = None,
    preferred_min_ram_gb: float = 0.0,
    budget_pct: "float | None" = None,
) -> "str | None":
    """Pick the best **in-process** MLX model id for this machine.

    Returns a HuggingFace
    repo id the caller loads directly via mlx_lm + outlines (FSM-constrained
    decoding). It deliberately does NOT discover external servers
    (Ollama / LM Studio / Apple Intelligence give no constrained decoding) and
    does NOT spawn a managed mlx_lm.server — the MLX classifier server loads the
    chosen model in-process and is the single LLM host for every stage.

    Priority:
      1. ``preferred_hf_id`` when it fits the Metal headroom budget — the
         eval-tuned classifier model. Keep it on capable machines; degrade only
         when it physically won't fit, so a generic catalog ``quality_score``
         never silently swaps out the tuned model on a big box.
      2. The largest catalog model that fits (``_select_mlx_entry``).
      3. ``preferred_hf_id`` as a best-effort fallback when nothing in the
         catalog fits — let ``mlx_lm.load`` try rather than returning None and
         breaking the load.

    Returns None only when no ``preferred_hf_id`` is given and nothing fits.
    """
    if budget_pct is None:
        try:
            from agents.config import LLM_BUDGET_PCT
            budget_pct = LLM_BUDGET_PCT
        except Exception:
            budget_pct = 0.5

    with _tracer.start_as_current_span("llm_selector.select_mlx_model_id") as span:
        span.set_attribute("llm.preferred_hf_id", preferred_hf_id or "")
        span.set_attribute("llm.preferred_min_ram_gb", preferred_min_ram_gb)
        span.set_attribute("llm.budget_pct", budget_pct)

        # Non-Apple-Silicon: no MLX runtime — return the preferred default so the
        # caller's behaviour is unchanged (the load path no-ops/fails as before).
        if platform.system() != "Darwin":
            span.set_attribute("llm.reason", "not_darwin")
            span.set_attribute("llm.selected_model", preferred_hf_id or "")
            return preferred_hf_id
        brand = _sysctl("machdep.cpu.brand_string") or ""
        if not brand.startswith("Apple M"):
            span.set_attribute("llm.reason", "not_apple_silicon")
            span.set_attribute("llm.selected_model", preferred_hf_id or "")
            return preferred_hf_id

        apple_intelligence = _apple_intelligence_available()

        try:
            snap = probe_compute()
        except Exception as exc:  # noqa: BLE001
            span.record_exception(exc)
            span.set_attribute("llm.reason", "compute_probe_failed")
            span.set_attribute("llm.selected_model", preferred_hf_id or "")
            log.warning("llm_selector: compute probe failed (%s) — using %s",
                        exc, preferred_hf_id)
            return preferred_hf_id

        # Relax the budget when the screen is locked — the user won't feel the
        # latency — and mirror _select_mlx_entry's heavy-throttle cap so the
        # preferred-fit check sees the same budget the catalog ladder does.
        effective_pct = (
            min(0.8, budget_pct * 1.5) if snap.screen_locked else budget_pct
        )
        budget = snap.metal_headroom_gb * effective_pct
        if snap.thermal_level >= 2:
            budget = min(budget, 9.0)

        span.set_attribute("llm.headroom_gb",   round(snap.metal_headroom_gb, 2))
        span.set_attribute("llm.effective_pct", round(effective_pct, 3))
        span.set_attribute("llm.budget_gb",     round(budget, 2))
        span.set_attribute("llm.thermal_level", snap.thermal_level)
        span.set_attribute("llm.screen_locked", snap.screen_locked)

        # 1. Keep the tuned classifier model when it fits.
        if preferred_hf_id and preferred_min_ram_gb <= budget:
            span.set_attribute("llm.reason", "preferred_fits")
            span.set_attribute("llm.selected_model", preferred_hf_id)
            log.info(
                "llm_selector: MLX in-process model=%s (preferred — min_ram=%.1f GB "
                "fits budget=%.1f GB)",
                preferred_hf_id, preferred_min_ram_gb, budget,
            )
            return preferred_hf_id

        # 2. Largest catalog model that BOTH fits the budget AND is already in
        #    the HF cache. Apple Intelligence (apple_fm, min_ram=0) is always
        #    "available" on supported machines — no HF cache check needed.
        #    Gating MLX entries on the cache keeps "dynamic" meaning "best
        #    among what's present" — never a surprise multi-GB download on
        #    constrained machines. The `budget` here is already thermal-capped.
        for model_id, backend, min_ram, quality, hf_id in _MODELS:
            if backend == "apple_fm":
                if apple_intelligence:
                    span.set_attribute("llm.reason", "apple_intelligence_catalog")
                    span.set_attribute("llm.selected_model", APPLE_INTELLIGENCE_ID)
                    log.info(
                        "llm_selector: MLX in-process fallback=Apple Intelligence "
                        "(no cached MLX model fits budget=%.1f GB)",
                        budget,
                    )
                    return APPLE_INTELLIGENCE_ID
                continue
            if min_ram > budget:
                continue
            if not _hf_model_cached(hf_id):
                log.debug(
                    "llm_selector: skip %s — fits budget=%.1f GB but not in HF cache",
                    hf_id, budget,
                )
                continue
            span.set_attribute("llm.reason", "catalog_fit_cached")
            span.set_attribute("llm.selected_model", hf_id)
            log.info(
                "llm_selector: MLX in-process model=%s hf=%s min_ram=%.1f GB "
                "quality=%d cached (preferred=%s did not fit budget=%.1f GB)",
                model_id, hf_id, min_ram, quality, preferred_hf_id, budget,
            )
            return hf_id

        # 3. Nothing cached fits and Apple Intelligence is unavailable (macOS < 26).
        #    Pick the largest catalog model that fits the budget, ignoring the cache
        #    (it will trigger a one-time download). This prevents returning an
        #    oversized preferred model (e.g. 6.5 GB Qwen3.5-9B on an 8 GB machine
        #    whose Metal budget is ~2.7 GB) that would exceed available memory.
        #    Only fall back to preferred_hf_id when nothing in the catalog fits.
        entry = _select_mlx_entry(snap.metal_headroom_gb, effective_pct,
                                  snap.thermal_level, apple_intelligence)
        if entry is not None:
            _, _, min_ram, _, hf_id = entry
            span.set_attribute("llm.reason", "catalog_fit_uncached")
            span.set_attribute("llm.selected_model", hf_id or "")
            log.warning(
                "llm_selector: no cached MLX model fits budget=%.1f GB — "
                "selecting %s (min_ram=%.1f GB fits; will download)",
                budget, hf_id, min_ram,
            )
            return hf_id
        span.set_attribute("llm.reason", "nothing_fits_use_preferred")
        span.set_attribute("llm.selected_model", preferred_hf_id or "")
        log.warning(
            "llm_selector: no catalog model fits budget=%.1f GB — "
            "last-resort fallback to preferred %s",
            budget, preferred_hf_id,
        )
        return preferred_hf_id


def resolve_model(name: str) -> "dict | None":
    """Resolve a short name ('phi-4') or HF ID to its catalog entry.

    Accepts either the short id from _MODELS (e.g. 'phi-4') or the full HF
    repo ID (e.g. 'mlx-community/phi-4-4bit'). Returns a dict with keys
    short_name, hf_id, backend, min_ram_gb, quality_score, or None if not found.
    """
    for model_id, backend, min_ram, quality, hf_id in _MODELS:
        if model_id == name or (hf_id and hf_id == name):
            return {
                "short_name":    model_id,
                "hf_id":         hf_id,
                "backend":       backend,
                "min_ram_gb":    min_ram,
                "quality_score": quality,
            }
    return None


def discover_mlx_eval_server(port: int = 7823) -> "str | None":
    """Return the base URL of a running MLX eval server, or None.

    Probes localhost:<port> with a TCP check then a /health probe. Only
    returns a URL if the server responds with backend='mlx'. Does not start
    or manage any server process.
    """
    if not _tcp_open("127.0.0.1", port):
        return None
    data, status = _get_json(f"http://127.0.0.1:{port}/health")
    if status == 200 and data and data.get("backend") == "mlx":
        return f"http://127.0.0.1:{port}"
    return None


__all__ = ["local_infer", "discover_running_servers", "probe_compute",
           "RunningServer", "ComputeSnapshot",
           "select_mlx_model_id",
           "resolve_model", "discover_mlx_eval_server"]
