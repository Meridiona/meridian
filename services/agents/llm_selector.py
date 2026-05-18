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
import signal
import socket
import subprocess
import sys
import time
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

log = logging.getLogger("agents.llm_selector")

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
    found: list[RunningServer] = []

    # 1. Ollama — unique port; /api/ps = currently loaded in VRAM
    if _tcp_open("127.0.0.1", 11434):
        ver, _ = _get_json("http://127.0.0.1:11434/api/version")
        if ver and "version" in ver:
            ps, _ = _get_json("http://127.0.0.1:11434/api/ps")
            models = [m["name"] for m in (ps or {}).get("models", [])]
            if models:
                found.append(RunningServer(
                    "ollama", "http://127.0.0.1:11434/v1", models, models[0]))
                log.info("llm_selector: Ollama loaded=%s", models)

    # 2. LM Studio — use native /api/v0/models (has state field) to distinguish
    #    in-memory models from installed-only ones. /v1/models omits state entirely.
    if _tcp_open("127.0.0.1", 1234):
        native, native_status = _get_json("http://127.0.0.1:1234/api/v0/models")
        if native_status == 200 and native:
            models = [m["id"] for m in native.get("data", [])
                      if m.get("state") == "loaded"]
        else:
            # Older LM Studio — fall back to /v1/models, take all listed models
            data, status = _get_json("http://127.0.0.1:1234/v1/models")
            models = [m["id"] for m in (data or {}).get("data", [])] if status == 200 else []
        if models:
            found.append(RunningServer(
                "lmstudio", "http://127.0.0.1:1234/v1", models, models[0]))
            log.info("llm_selector: LM Studio loaded=%s", models)

    # 3. Port 8080 — llama.cpp or mlx_lm; /props 200 = llama.cpp, 404 = mlx_lm
    if _tcp_open("127.0.0.1", 8080):
        props, props_status = _get_json("http://127.0.0.1:8080/props")
        if props_status == 200 and props:
            data, _ = _get_json("http://127.0.0.1:8080/v1/models")
            models = [m["id"] for m in (data or {}).get("data", [])]
            if models:
                found.append(RunningServer(
                    "llamacpp", "http://127.0.0.1:8080/v1", models, models[0]))
                log.info("llm_selector: llama.cpp loaded=%s", models)
        else:
            data, status = _get_json("http://127.0.0.1:8080/v1/models")
            if status == 200 and data:
                models = [m["id"] for m in data.get("data", [])]
                if models:
                    found.append(RunningServer(
                        "mlxlm", "http://127.0.0.1:8080/v1", models, models[0]))
                    log.info("llm_selector: mlx_lm server loaded=%s", models)

    # 4. Apple FoundationModels — in-process, no port, macOS 26+
    try:
        from apple_fm_sdk import SystemLanguageModel  # type: ignore[import]
        if SystemLanguageModel.default.is_available()[0]:
            found.append(RunningServer(
                "apple_fm", "", ["apple-intelligence"], "apple-intelligence"))
            log.info("llm_selector: Apple Intelligence available")
    except ImportError:
        pass

    return found


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


@dataclass
class LocalModelEndpoint:
    model: str       # model name to pass to AIAgent
    base_url: str    # OpenAI-compatible base URL
    api_key: str     # typically "local"
    runtime: str     # "ollama" | "lmstudio" | "llamacpp" | "mlxlm" | "mlx_managed"


_MANAGED_SERVER_PORT = 8765
_MANAGED_SERVER_PID_FILE = Path.home() / ".meridian" / "mlx_lm_server.pid"


def _metal_headroom_gb() -> float:
    """Primary memory signal — headroom within Metal's recommended working set."""
    try:
        import mlx.core as mx  # type: ignore[import]
        info    = mx.device_info()
        ceiling = info["max_recommended_working_set_size"]
        active  = mx.get_active_memory()
        cached  = mx.get_cache_memory()
        return (ceiling - active - cached) / (1 << 30)
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
        return (pg("Pages free") + pg("Pages inactive")) * page_size / (1 << 30)
    except Exception:
        return 0.0


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
    import psutil  # type: ignore[import]
    brand = _sysctl("machdep.cpu.brand_string") or ""
    key   = re.sub(r"\s+", " ", brand).lower()
    _, mem_bw = _CHIP_SPECS.get(key, (None, 0))
    return ComputeSnapshot(
        metal_headroom_gb=_metal_headroom_gb(),
        thermal_level=_thermal_level(),
        cpu_pct=psutil.cpu_percent(interval=0.5),
        screen_locked=_screen_locked(),
        chip_name=brand,
        mem_bw_gbs=mem_bw or 0,
    )


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
    (caller falls back to the cloud AIAgent path).

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

    macos_major = int(platform.mac_ver()[0].split(".")[0] or "0")
    apple_intelligence = macos_major >= 26

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
            session = LanguageModelSession(system_prompt=system)
            r = await session.respond(prompt=user)
            return r.content
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


def _ensure_mlx_server(model_id: str, port: int = _MANAGED_SERVER_PORT) -> bool:
    pid_file = _MANAGED_SERVER_PID_FILE
    if pid_file.exists():
        try:
            meta = json.loads(pid_file.read_text())
            pid, existing_model, existing_port = meta["pid"], meta["model"], meta["port"]
            try:
                os.kill(pid, 0)
                alive = True
            except OSError:
                alive = False
            if alive and existing_model == model_id and existing_port == port:
                return True
            if alive:
                os.kill(pid, signal.SIGTERM)
                time.sleep(3)
        except Exception:
            pass

    proc = subprocess.Popen(
        [sys.executable, "-m", "mlx_lm.server",
         "--model", model_id, "--port", str(port), "--max-tokens", "4096"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    pid_file.parent.mkdir(parents=True, exist_ok=True)
    pid_file.write_text(json.dumps({"pid": proc.pid, "model": model_id, "port": port}))

    url = f"http://127.0.0.1:{port}/v1/models"
    deadline = time.monotonic() + 90.0
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            log.warning("llm_selector: mlx_lm.server exited early (exit=%d) — is mlx_lm installed?",
                        proc.returncode)
            pid_file.unlink(missing_ok=True)
            return False
        _, status = _get_json(url, timeout=1.0)
        if status == 200:
            log.info("llm_selector: mlx_lm.server ready model=%s port=%d", model_id, port)
            return True
        time.sleep(1)
    log.warning("llm_selector: mlx_lm.server startup timeout model=%s", model_id)
    return False


def select_model_for_hermes(budget_pct: float = 0.5) -> Optional[LocalModelEndpoint]:
    """Return the best available local endpoint for AIAgent, or None to use cloud."""
    if platform.system() != "Darwin":
        return None
    brand = _sysctl("machdep.cpu.brand_string") or ""
    if not brand.startswith("Apple M"):
        return None

    for server in discover_running_servers():
        if server.runtime == "apple_fm":
            continue
        log.info("llm_selector: hermes endpoint reusing %s model=%s",
                 server.runtime, server.best_model)
        return LocalModelEndpoint(
            model=server.best_model,
            base_url=server.base_url,
            api_key="local",
            runtime=server.runtime,
        )

    try:
        snap = probe_compute()
    except Exception as exc:
        log.warning("llm_selector: compute probe failed: %s", exc)
        return None

    effective_pct = min(0.8, budget_pct * 1.5) if snap.screen_locked else budget_pct
    entry = _select_mlx_entry(snap.metal_headroom_gb, effective_pct,
                              snap.thermal_level, apple_intelligence=False)
    if entry is None:
        log.info("llm_selector: no local model fits budget headroom=%.1f GB pct=%.2f",
                 snap.metal_headroom_gb, effective_pct)
        return None

    model_id, _, _, _, hf_id = entry
    log.info("llm_selector: hermes will use mlx_managed model=%s hf=%s", model_id, hf_id)
    if _ensure_mlx_server(hf_id, _MANAGED_SERVER_PORT):
        return LocalModelEndpoint(
            model=hf_id,
            base_url=f"http://127.0.0.1:{_MANAGED_SERVER_PORT}/v1",
            api_key="local",
            runtime="mlx_managed",
        )
    return None


__all__ = ["local_infer", "discover_running_servers", "probe_compute",
           "RunningServer", "ComputeSnapshot", "LocalModelEndpoint",
           "select_model_for_hermes"]
