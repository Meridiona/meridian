//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// MLX classifier server health (L2). Distinguishes liveness (port open) from
// readiness (model actually loaded) — the latter is the gap the Rust preflight
// historically missed. Content-free: only status codes and model metadata.

use crate::config::Config;
use crate::health::Check;
use std::time::Duration;

pub async fn checks(cfg: &Config) -> Vec<Check> {
    let base = format!("http://127.0.0.1:{}", cfg.mlx_server_port);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return vec![Check::warn(
                "mlx client",
                "L2",
                format!("http client error ({e})"),
            )]
        }
    };

    let mut out = Vec::new();

    // 1. liveness + backend via /health
    match client.get(format!("{base}/health")).send().await {
        Err(_) => {
            out.push(
                Check::critical("reachable", "L2", format!("MLX server down at {base}"))
                    .with_remedy("meridian start (or check the mlx-server launchd agent / port)"),
            );
            return out; // nothing else to probe if it is down
        }
        Ok(resp) => {
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            out.push(Check::ok(
                "reachable",
                "L2",
                format!("/health {}", status.as_u16()),
            ));
            let backend = body.get("backend").and_then(|v| v.as_str()).unwrap_or("");
            if backend == "mlx" {
                out.push(Check::ok("backend", "L2", "mlx"));
            } else {
                out.push(
                    Check::warn(
                        "backend",
                        "L2",
                        format!("'{backend}' (expected mlx) — MLX endpoints will 503"),
                    )
                    .with_remedy("start the server with --backend mlx"),
                );
            }
        }
    }

    // 2. readiness via /info — server up + current model state.
    // Lazy-load builds load the model on first request and EVICT it after
    // MLX_IDLE_EVICT_S idle, so "not resident" is a healthy idle state, not an
    // error. `loaded_at` means the server is up; `model_resident`/`active_memory_gb`
    // (lazy-load builds only) report the live footprint — `ps` cannot see it.
    match client.get(format!("{base}/info")).send().await {
        Ok(resp) => {
            let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            let model = body.get("model_id").and_then(|v| v.as_str());
            let server_ready = body.get("loaded_at").map(|v| !v.is_null()).unwrap_or(false);
            // Present on lazy-load builds; absent on older eager builds.
            let resident = body.get("model_resident").and_then(|v| v.as_bool());
            let active_gb = body.get("active_memory_gb").and_then(|v| v.as_f64());
            match (model, server_ready) {
                (Some(m), true) => {
                    // Apple Intelligence is an on-device backend with no MLX model to
                    // load or evict — model_resident is always false there, so the
                    // "idle (evicted)" wording would be misleading.
                    let detail = if m == "apple-intelligence" {
                        format!("{m} — on-device (no MLX model)")
                    } else {
                        match (resident, active_gb) {
                            (Some(true), Some(gb)) => format!("{m} — resident, {gb:.1} GB"),
                            (Some(true), None) => format!("{m} — resident"),
                            (Some(false), _) => format!("{m} — idle (evicted; loads on demand)"),
                            (None, _) => m.to_string(), // older eager build: loaded_at ⇒ resident
                        }
                    };
                    out.push(Check::ok("model server", "L2", detail));
                }
                (Some(m), false) => out.push(
                    Check::warn("model server", "L2", format!("{m} still loading"))
                        .with_remedy("wait for the model load to finish, then re-run"),
                ),
                (None, _) => out.push(
                    Check::warn("model server", "L2", "model not reported").with_remedy(
                        "restart the mlx-server; check its logs for an OOM/load error",
                    ),
                ),
            }
        }
        Err(_) => out.push(Check::info(
            "model server",
            "L2",
            "/info not exposed by this build",
        )),
    }
    out
}
