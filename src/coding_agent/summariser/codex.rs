// meridian — normalises screenpipe activity into structured app sessions
//
// Run `codex exec` to summarise a Codex session (symmetry with claude.rs). Safe,
// side-effect-free, non-interactive: `-s read-only`, `--skip-git-repo-check`,
// `--ephemeral` (no session file → indexer won't re-pick it), `--output-schema`
// + `-o FILE` to capture the structured final message. Port of
// services/coding_agent_summariser/codex_runner.py.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::config::SummariserConfig;
use super::prompts;
use super::{run_capture, EngineOutput, SummariserError};

pub async fn run_codex(
    stdin_text: &str,
    cfg: &SummariserConfig,
) -> Result<EngineOutput, SummariserError> {
    let prompt = format!(
        "{} Summarise the coding-session transcript provided on stdin.",
        prompts::summary_instruction()
    );

    // Unique scratch dir for the schema + captured final message. Avoids the
    // time/random APIs (banned in some contexts) via pid + a static counter.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let td = std::env::temp_dir().join(format!(
        "codex_summ_{}_{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    if let Err(e) = std::fs::create_dir_all(&td) {
        return Err(SummariserError::Failed(format!("codex: temp dir: {e}")));
    }
    let _guard = TempDirGuard(td.clone());
    let schema_path = td.join("schema.json");
    let out_path = td.join("last_message.txt");
    if let Err(e) = std::fs::write(&schema_path, prompts::summary_schema_json()) {
        return Err(SummariserError::Failed(format!("codex: write schema: {e}")));
    }

    let home = cfg.meridian_home.display().to_string();
    let mut args: Vec<String> = vec![
        "exec".into(),
        prompt,
        "-s".into(),
        "read-only".into(),
        "--skip-git-repo-check".into(),
        "--ephemeral".into(),
        "--output-schema".into(),
        schema_path.display().to_string(),
        "-o".into(),
        out_path.display().to_string(),
        "-C".into(),
        home,
    ];
    if !cfg.codex_model.is_empty() {
        args.push("-m".into());
        args.push(cfg.codex_model.clone());
    }

    let cap = run_capture(
        "codex",
        &args,
        stdin_text,
        &cfg.meridian_home,
        cfg.codex_timeout_s,
        &[("MERIDIAN_SUMMARISER", "1")],
        &[],
    )
    .await?;

    if !cap.success {
        let blob = format!("{}\n{}", cap.stderr, cap.stdout);
        if prompts::looks_rate_limited(&blob) {
            let msg = prompts::first_line(&cap.stderr);
            return Err(SummariserError::RateLimited(if msg.is_empty() {
                "codex usage limit".into()
            } else {
                msg
            }));
        }
        return Err(SummariserError::Failed(format!(
            "codex exited {:?}: {}",
            cap.code,
            prompts::first_line(&cap.stderr)
        )));
    }

    let text = std::fs::read_to_string(&out_path).unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return Err(SummariserError::Failed("codex produced no output".into()));
    }

    let (summary, blockers) = extract(text);
    if summary.is_empty() {
        return Err(SummariserError::Failed(
            "codex output had no usable summary".into(),
        ));
    }
    Ok(EngineOutput { summary, blockers })
}

/// Pull (summary, blockers) from codex's final message. With --output-schema it
/// should be a JSON object; if codex returns prose, treat the whole text as the
/// summary.
fn extract(text: &str) -> (String, Vec<String>) {
    if let Some(obj) = try_json_object(text) {
        if let Some(summary) = obj.get("summary").and_then(Value::as_str) {
            let blockers = obj
                .get("blockers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|b| b.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            return (summary.trim().to_string(), blockers);
        }
    }
    (text.trim().to_string(), Vec::new())
}

fn try_json_object(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Some(v);
    }
    // Tolerate a JSON object embedded in surrounding prose.
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    if start < end {
        serde_json::from_str::<Value>(&text[start..=end]).ok()
    } else {
        None
    }
}

/// Best-effort recursive cleanup of the scratch dir on scope exit.
struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
