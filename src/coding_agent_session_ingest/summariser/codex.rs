//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Run `codex exec` to summarise a Codex session (symmetry with claude.rs). Safe,
// side-effect-free, non-interactive: `-s read-only`, `--skip-git-repo-check`,
// `--ephemeral` (no session file → indexer won't re-pick it), `--output-schema`
// + `-o FILE` to capture the structured final message. Port of
// the former Python summariser/codex_runner.py.
//
// # The whole prompt goes over stdin, not argv (Windows)
//
// `codex` resolves to `codex.cmd` on Windows - an npm-generated batch file, not a
// native exe - and Rust's std library refuses to spawn a `.bat`/`.cmd` target when an
// argument contains characters it cannot safely escape (the CVE-2024-24576 "BatBadBut"
// fix), notably embedded newlines. The instructions prompt is sourced from a Markdown
// rules file (`SUMMARY_RULES`), so it is always multi-line - meaning every real call
// through this function failed to even spawn on Windows with `io::Error { InvalidInput,
// "batch file arguments are invalid" }` (confirmed live:
// github.com/Meridiona/meridian/issues/805). `crate::llm::codex::CodexBackend` (the
// hourly worklog pipeline / connectivity test) already moved off argv for exactly this
// reason - this applies the same fix here, which had the identical bug all along.
// `codex exec` with no positional prompt reads the whole prompt from stdin instead.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::config::SummariserConfig;
use super::prompts;
use super::{run_capture, EngineOutput, SummariserError};

/// The instructions + the session transcript, combined into the one blob `codex exec`
/// reads from stdin now that no positional prompt is passed - see the module doc.
fn codex_stdin_payload(stdin_text: &str) -> String {
    let instructions = format!(
        "{} Summarise the coding-session transcript provided on stdin.",
        prompts::summary_instruction()
    );
    format!("{instructions}\n\n<stdin>\n{stdin_text}\n</stdin>\n")
}

/// The `codex exec` argv - no prompt in here, see the module doc. Split out so the
/// no-newline invariant this function exists to guarantee is directly testable.
fn codex_args(
    schema_path: &std::path::Path,
    out_path: &std::path::Path,
    home: String,
    model: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "exec".into(),
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
    if !model.is_empty() {
        args.push("-m".into());
        args.push(model.to_string());
    }
    args
}

pub async fn run_codex(
    stdin_text: &str,
    cfg: &SummariserConfig,
) -> Result<EngineOutput, SummariserError> {
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
    // `strictify`: matches `crate::llm::codex::CodexBackend`, which applies the same
    // transform before handing a schema to codex - without it, codex's own strict
    // dialect check can reject the schema with `invalid_json_schema` (see that
    // module's test fixture, a live 400 from exactly this).
    let schema = crate::llm::schema::strictify(&prompts::summary_schema_value());
    if let Err(e) = std::fs::write(&schema_path, schema.to_string()) {
        return Err(SummariserError::Failed(format!("codex: write schema: {e}")));
    }

    let home = cfg.meridian_home.display().to_string();
    let args = codex_args(&schema_path, &out_path, home, &cfg.codex_model);

    let cap = run_capture(
        "codex",
        &args,
        &codex_stdin_payload(stdin_text),
        &cfg.meridian_home,
        cfg.codex_timeout_s,
        &[("MERIDIAN_SUMMARISER", "1")],
        &[],
    )
    .await?;

    if !cap.success {
        let blob = format!("{}\n{}", cap.stderr, cap.stdout);
        if prompts::looks_rate_limited(&blob) {
            let msg = prompts::rate_limited_line(&blob)
                .unwrap_or_else(|| prompts::first_line(&cap.stderr));
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

    let summary = prompts::extract_summary(text);
    if summary.is_empty() {
        return Err(SummariserError::Failed(
            "codex output had no usable summary".into(),
        ));
    }
    Ok(EngineOutput { summary })
}

/// Best-effort recursive cleanup of the scratch dir on scope exit.
struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The premise the whole fix rests on: the instructions are sourced from a Markdown
    /// rules file and are always multi-line. If `SKILL.md` ever became single-line, this
    /// fix would no longer be guarding against anything real - this pins that it still is.
    #[test]
    fn the_instructions_prompt_is_multi_line() {
        assert!(prompts::summary_instruction().contains('\n'));
    }

    /// The regression this fix exists to close: `codex exec` argv must never carry the
    /// instructions prompt (or anything else newline-bearing) - that is exactly what makes
    /// Rust's std refuse to spawn `codex.cmd` on Windows. A future edit that reintroduces
    /// the prompt into `args` fails this test.
    #[test]
    fn codex_args_never_contains_a_newline() {
        let args = codex_args(
            Path::new("/tmp/schema.json"),
            Path::new("/tmp/out.txt"),
            "/tmp/home".into(),
            "gpt-5.5",
        );
        for arg in &args {
            assert!(!arg.contains('\n'), "argv entry carries a newline: {arg:?}");
        }
    }

    #[test]
    fn codex_args_omits_the_model_flag_when_unset() {
        let args = codex_args(
            Path::new("/tmp/schema.json"),
            Path::new("/tmp/out.txt"),
            "/tmp/home".into(),
            "",
        );
        assert!(!args.contains(&"-m".to_string()));
    }

    #[test]
    fn codex_args_carries_the_model_flag_when_set() {
        let args = codex_args(
            Path::new("/tmp/schema.json"),
            Path::new("/tmp/out.txt"),
            "/tmp/home".into(),
            "gpt-5.5",
        );
        let i = args
            .iter()
            .position(|a| a == "-m")
            .expect("-m flag present");
        assert_eq!(args[i + 1], "gpt-5.5");
    }

    /// `codex exec`'s argv must carry NO positional prompt - the instructions now live
    /// entirely in the stdin payload built by `codex_stdin_payload`.
    #[test]
    fn codex_args_has_no_positional_prompt() {
        let args = codex_args(
            Path::new("/tmp/schema.json"),
            Path::new("/tmp/out.txt"),
            "/tmp/home".into(),
            "",
        );
        assert_eq!(args[0], "exec");
        assert_eq!(
            args[1], "-s",
            "the second argv entry must be a flag, not a prompt"
        );
    }

    #[test]
    fn codex_stdin_payload_carries_both_the_instructions_and_the_transcript() {
        let payload = codex_stdin_payload("the transcript");
        assert!(payload.contains("the transcript"));
        assert!(payload.starts_with(&prompts::summary_instruction()[..40]));
    }
}
