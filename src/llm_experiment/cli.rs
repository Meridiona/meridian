//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! `meridian llm-experiment <run|create|exec|list|get|draft-task>` — the LLM-Lab CLI surface.
//!
//! * `run    --process hour-report --hour 2026-07-15T14 --variants claude,codex:gpt-5.1,local`
//!   — create + execute + print the detail JSON (the human one-shot). A `custom:<endpoint-id>`
//!   token runs one of the user's configured cloud endpoints (the id from settings.json's
//!   `custom_llm_providers`); its model comes from the endpoint, not a `:model` suffix.
//! * `run    --process worklog-generate --day 2026-07-15 --task-id T2 --variants …`
//! * `create <same args>` — insert the experiment + pending rows, print
//!   `{"experiment_id":N}`, exit. The tray uses this to get a handle fast, then…
//! * `exec   --id N` — run the pending variants (spawned detached by the tray so the
//!   UI polls progress instead of holding a long invoke). Resumable.
//! * `list   [--limit N]` / `get --id N` — read back via
//!   `meridian_core::llm_experiments`, printed as one JSON line.
//! * `draft-task --day D --variant p[:model] --task-json '{…}'` — the ONE exception
//!   to the paragraph below: an ephemeral, on-demand worklog draft for a single
//!   inline task that fires a REAL, metered LLM completion (see [`draft_task`]).
//!
//! `run`/`create`/`exec`/`list`/`get` are deliberately ungated in release binaries:
//! they are useful for field debugging and **only ever write the experiment
//! tables** — no network, no cost — so an unattended run is harmless. `draft-task`
//! is different (it calls a live provider and can incur cost), so it alone is
//! **dev-gated** here too — a release binary refuses it, matching the tray's
//! `draft_lab_worklog` gate. The UI surface as a whole is the gated part.
//!
//! # Who calls this
//! `main.rs`'s `llm-experiment` dispatch block; the tray's `run_llm_experiment`
//! (and, for `draft-task`, `draft_lab_worklog`).

use anyhow::{bail, Context, Result};
use sqlx::SqlitePool;

use super::{
    parse_variants, request, runner, store, ExperimentInput, ExperimentProcess, ExperimentSpec,
    Variant,
};

const USAGE: &str = "usage: meridian llm-experiment \
run|create --process hour-report|workstream-fold|worklog-generate|day-fold \
(--hour YYYY-MM-DDTHH | --day YYYY-MM-DD [--task-id T]) --variants p1,p2:model,custom:id,… \
| exec --id N | list [--limit N] | get --id N \
| draft-task --day YYYY-MM-DD --variant p[:model] --task-json '{\"title\":…,\"summary\":[…],\"minutes\":N}'";

/// Parse argv and dispatch. Prints exactly one JSON line on success (except
/// `create`'s `{"experiment_id":N}` — also one line); errors bubble to `main.rs`.
pub async fn run(pool: &SqlitePool) -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let verb = args.get(2).map(String::as_str).unwrap_or("");

    match verb {
        "run" => {
            let spec = parse_spec(&args)?;
            let id = create_experiment(pool, &spec).await?;
            runner::exec(pool, id).await?;
            print_detail(pool, id).await
        }
        "create" => {
            let spec = parse_spec(&args)?;
            let id = create_experiment(pool, &spec).await?;
            println!("{{\"experiment_id\":{id}}}");
            Ok(())
        }
        "exec" => {
            let id = flag_i64(&args, "--id")?;
            runner::exec(pool, id).await?;
            print_detail(pool, id).await
        }
        "list" => {
            let limit = flag(&args, "--limit")
                .map(|v| v.parse::<i64>().context("--limit must be a number"))
                .transpose()?
                .unwrap_or(20);
            let rows = meridian_core::llm_experiments::list_experiments(pool, limit).await?;
            println!("{}", serde_json::to_string(&rows)?);
            Ok(())
        }
        "get" => {
            let id = flag_i64(&args, "--id")?;
            print_detail(pool, id).await
        }
        "draft-task" => draft_task(pool, &args).await,
        _ => bail!("{USAGE}"),
    }
}

/// Ephemeral inline worklog draft for the dev-only LLM Lab: draft ONE fold task
/// with ONE variant, print `{"draft": <raw model answer>}`, and persist nothing
/// (no experiment row). The task content is passed inline
/// (`--task-json {title,summary,minutes}`) because a fold task id is a model's
/// SIMULATED day, not a production `day_tasks` row - so there is nothing to read
/// by id. Fires a real, metered completion; the UI gates this behind a
/// free/local caution. The tray `draft_lab_worklog` command shells out to it.
///
/// Unlike its sibling subcommands (which only write local experiment tables), this
/// one costs money, so it is refused in a release build - it runs only from a dev
/// binary, the same `debug_assertions` signal the tray's `dev_only` gate uses.
async fn draft_task(pool: &SqlitePool, args: &[String]) -> Result<()> {
    if !cfg!(debug_assertions) {
        bail!(
            "draft-task is a dev-only subcommand - it fires a real, metered LLM completion, \
             so a release binary refuses it"
        );
    }
    let day = flag(args, "--day").with_context(|| format!("draft-task needs --day - {USAGE}"))?;
    let variant =
        flag(args, "--variant").with_context(|| format!("draft-task needs --variant - {USAGE}"))?;
    let task_json = flag(args, "--task-json")
        .with_context(|| format!("draft-task needs --task-json - {USAGE}"))?;

    #[derive(serde::Deserialize)]
    struct InlineTask {
        title: String,
        #[serde(default)]
        summary: Vec<String>,
        #[serde(default)]
        minutes: i64,
    }
    let task: InlineTask = serde_json::from_str(&task_json).context("parsing --task-json")?;

    // Parse the variant token (validates the provider + the custom:<id> shape),
    // then resolve its backend from live settings - the same resolution the
    // experiment runner uses, so a `custom:<id>` draft hits the same endpoint.
    let v = Variant::parse(&variant)?;
    let model = v.model.unwrap_or_default();
    let backend =
        runner::resolve_backend(v.provider.as_str(), &model).map_err(anyhow::Error::msg)?;

    let (req, _) = crate::pm_worklog::generate::generate_request_from_task(
        pool,
        &day,
        task.title,
        task.summary,
        task.minutes,
    )
    .await?;

    let out = backend
        .complete(&req)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    println!("{}", serde_json::json!({ "draft": out.text }));
    Ok(())
}

/// Build + snapshot + insert; returns the new experiment id.
async fn create_experiment(pool: &SqlitePool, spec: &ExperimentSpec) -> Result<i64> {
    let input_json = request::build_input_json(pool, spec).await?;
    let now = chrono::Utc::now().to_rfc3339();
    store::create(pool, spec, &input_json, &now).await
}

/// Print an experiment's detail JSON (or `null` for an unknown id) on one line.
async fn print_detail(pool: &SqlitePool, id: i64) -> Result<()> {
    let detail = meridian_core::llm_experiments::get_experiment(pool, id).await?;
    println!("{}", serde_json::to_string(&detail)?);
    Ok(())
}

/// `--process` + input flags + `--variants` → an [`ExperimentSpec`].
fn parse_spec(args: &[String]) -> Result<ExperimentSpec> {
    let process_str = flag(args, "--process").with_context(|| USAGE.to_string())?;
    let process = ExperimentProcess::from_wire(&process_str)
        .with_context(|| format!("unknown process {process_str:?} - {USAGE}"))?;

    let input = match process {
        ExperimentProcess::HourReport | ExperimentProcess::WorkstreamFold => {
            let hour = flag(args, "--hour")
                .with_context(|| format!("{} needs --hour YYYY-MM-DDTHH", process.as_str()))?;
            ExperimentInput::Hour(hour)
        }
        ExperimentProcess::WorklogGenerate => {
            let day = flag(args, "--day")
                .with_context(|| "worklog-generate needs --day YYYY-MM-DD".to_string())?;
            let task_id = flag(args, "--task-id")
                .with_context(|| "worklog-generate needs --task-id".to_string())?;
            ExperimentInput::DayTask { day, task_id }
        }
        ExperimentProcess::DayFold => ExperimentInput::Day(
            flag(args, "--day").with_context(|| "day-fold needs --day YYYY-MM-DD".to_string())?,
        ),
    };

    let variants = parse_variants(
        &flag(args, "--variants").with_context(|| "missing --variants".to_string())?,
    )?;
    Ok(ExperimentSpec {
        process,
        input,
        variants,
    })
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn flag_i64(args: &[String], name: &str) -> Result<i64> {
    flag(args, name)
        .with_context(|| format!("missing {name} - {USAGE}"))?
        .parse::<i64>()
        .with_context(|| format!("{name} must be a number"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmProvider;

    fn argv(rest: &str) -> Vec<String> {
        let mut v = vec!["meridian".to_string(), "llm-experiment".to_string()];
        v.extend(rest.split_whitespace().map(String::from));
        v
    }

    #[test]
    fn parses_an_hour_process_spec() {
        let spec = parse_spec(&argv(
            "run --process hour-report --hour 2026-07-15T14 --variants claude,local",
        ))
        .unwrap();
        assert_eq!(spec.process, ExperimentProcess::HourReport);
        assert_eq!(spec.input.ref_str(), "2026-07-15T14");
        assert_eq!(spec.variants.len(), 2);
        assert_eq!(spec.variants[0].provider, LlmProvider::Claude);
    }

    #[test]
    fn parses_a_day_task_spec_and_rejects_missing_flags() {
        let spec = parse_spec(&argv(
            "run --process worklog-generate --day 2026-07-15 --task-id T2 --variants codex:gpt-5.1",
        ))
        .unwrap();
        assert_eq!(spec.input.ref_str(), "2026-07-15/T2");
        assert_eq!(spec.variants[0].model.as_deref(), Some("gpt-5.1"));

        assert!(parse_spec(&argv("run --process worklog-generate --variants local")).is_err());
        assert!(parse_spec(&argv("run --process hour-report --variants local")).is_err());
        assert!(parse_spec(&argv("run --hour 2026-07-15T14 --variants local")).is_err());
    }
}
