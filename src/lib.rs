//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

/// Process-wide allocator. mimalloc actively returns freed pages to the OS instead of
/// retaining them for reuse (the platform-default behavior on macOS/Windows/Linux alike),
/// so RSS tracks actual live memory rather than the high-water mark of the largest
/// transient allocation. Matters most for the embedder ([`embedder`]), which frees a full
/// batch of tensors after every chunk — without this, the freed pages stay resident and
/// RSS never comes back down between chunks or after the model unloads.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod cli_flags;
pub mod coding_agent_session_ingest;
pub mod config;
pub mod daily_plan;
pub mod day_summary;
pub mod db;
pub mod embedder;
pub mod errors;
pub mod etl;
pub mod health;
pub mod intelligence;
pub mod llm;
pub mod llm_experiment;
pub mod notices;
pub mod notification_responses;
pub mod notifications;
pub mod observability;
pub mod plan_tasks;
pub mod platform;
pub mod pm_worklog;
pub mod restart;
pub mod telemetry_spool;
pub mod uninstall;
pub mod worklog_pipeline;

/// Test-only. Crate-wide coordination for the process-global
/// `MERIDIAN_SETTINGS_PATH`, which more than one module's tests manipulate -
/// see the module docs for the bug that per-module locks caused.
#[cfg(test)]
pub(crate) mod test_env;
