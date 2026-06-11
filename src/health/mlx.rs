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
                        format!("'{backend}' (expected mlx) — /classify_sessions will 503"),
                    )
                    .with_remedy("start the server with --backend mlx"),
                );
            }
        }
    }

    // 2. readiness via /info — model actually loaded, not just port open
    match client.get(format!("{base}/info")).send().await {
        Ok(resp) => {
            let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            let model = body.get("model_id").and_then(|v| v.as_str());
            let loaded = body.get("loaded_at").map(|v| !v.is_null()).unwrap_or(false);
            match (model, loaded) {
                (Some(m), true) => out.push(Check::ok("model loaded", "L2", m.to_string())),
                (Some(m), false) => out.push(
                    Check::warn("model loaded", "L2", format!("{m} still loading"))
                        .with_remedy("wait for the model load to finish, then re-run"),
                ),
                (None, _) => out.push(
                    Check::warn("model loaded", "L2", "model not reported as loaded").with_remedy(
                        "restart the mlx-server; check its logs for an OOM/load error",
                    ),
                ),
            }
        }
        Err(_) => out.push(Check::info(
            "model loaded",
            "L2",
            "/info not exposed by this build",
        )),
    }
    out
}
