//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Last-resort summarisation via the user's globally chosen AI provider.
//!
//! # The rule this implements
//!
//! A coding-agent transcript is summarised by **its own CLI** - a Codex session by `codex`,
//! a Cursor session by `cursor-agent`. That agent already holds the context and its
//! subscription is already paid for, so it is always tried first, `primary_attempts` times.
//!
//! This module is what happens *after* those attempts are exhausted: rather than leaving
//! the row to be dead-lettered, the segment is summarised by whatever provider the user
//! picked in Settings ([`crate::llm::resolver`]). Usually that is the same CLI, in which
//! case we do not call at all (see [`try_summarise`]) - but it need not be, and when it
//! differs it is the difference between a summarised hour and a lost one.
//!
//! # A rate limit is NOT a permanent failure
//!
//! This is the load-bearing distinction. A quota that refused us will refill, so a
//! rate-limited primary must be *waited out*, never routed around: the caller breaks out
//! before reaching this module, backs the source off, and retries on a later tick. Only a
//! `Failed` primary - crashed, not installed, signed out, subscription ended, output
//! unusable - reaches here, and only after every retry is spent. Falling back on a rate
//! limit would spend a second subscription's quota to dodge a wait of minutes, and would
//! silently attribute the work to a provider whose turn it was not.
//!
//! # Related
//! - [`super::summarise_one`] - the caller; owns the primary attempt loop and the ordering
//! - [`crate::llm::resolver::complete`] - the global provider call, with its own retry and
//!   rate-limit backoff (this module adds none of its own)

use crate::llm::{LlmError, LlmProvider, PromptRequest};

use super::{prompts, Source, SummariserError};

/// Which provider a primary engine corresponds to, so an identical fallback can be skipped.
///
/// The two vocabularies were deliberately kept in sync (see [`LlmProvider`]'s wire forms),
/// and this is the one place that bridges them.
fn provider_of(source: Source) -> Option<LlmProvider> {
    match source {
        Source::Claude => Some(LlmProvider::Claude),
        Source::Codex => Some(LlmProvider::Codex),
        Source::Copilot => Some(LlmProvider::Copilot),
        Source::CursorAgent => Some(LlmProvider::Cursor),
        Source::Mlx | Source::None | Source::Fallback(_) => None,
    }
}

/// Summarise `prompt` with the user's globally chosen provider.
///
/// Returns `Ok(None)` - not an error - when the chosen provider IS the engine that just
/// failed. Re-running the same CLI a third time would fail the same way; the row is better
/// left pending for a tick when that CLI is healthy again.
///
/// `label` is the trace label (the session uuid), never the model.
#[tracing::instrument(skip(prompt), fields(primary = primary.as_str(), provider = tracing::field::Empty))]
pub async fn try_summarise(
    prompt: &str,
    label: &str,
    primary: Source,
) -> Result<Option<(String, LlmProvider)>, SummariserError> {
    let chosen = crate::llm::resolver::chosen_provider();
    tracing::Span::current().record("provider", chosen.as_str());

    if provider_of(primary) == Some(chosen) {
        tracing::debug!(
            provider = chosen.as_str(),
            "summariser fallback skipped - the global provider is the engine that just failed"
        );
        return Ok(None);
    }

    tracing::info!(
        provider = chosen.as_str(),
        primary = primary.as_str(),
        "summariser falling back to the global AI provider - the session's own CLI failed every attempt"
    );

    let mut req = PromptRequest::new(
        prompts::SUMMARY_RULES,
        prompt.to_string(),
        label.to_string(),
    );
    // Schema-capable backends (claude/codex) enforce it; the rest carry it in the prompt and
    // `extract_summary` unwraps whichever shape comes back.
    if let Ok(schema) = serde_json::from_str(&prompts::summary_schema_json()) {
        req = req.with_schema(schema);
    }

    match crate::llm::resolver::complete(&req).await {
        Ok((out, provider)) => {
            let summary = prompts::extract_summary(&out.text);
            if summary.is_empty() {
                return Err(SummariserError::Failed(format!(
                    "{} fallback returned no usable summary",
                    provider.as_str()
                )));
            }
            Ok(Some((summary, provider)))
        }
        // Surfaced as `Failed`, deliberately: the caller's `rate_limited` flag drives the
        // per-AGENT-SOURCE backoff, and the fallback provider is a different account
        // entirely. Parking the agent's own queue because a substitute ran out of quota
        // would punish the wrong subscription. The provider-level backoff inside
        // `resolver::complete` already stops us dialling it again until it resets.
        Err(LlmError::RateLimited { message, .. }) => Err(SummariserError::Failed(format!(
            "{} fallback rate-limited: {message}",
            chosen.as_str()
        ))),
        Err(e) => Err(SummariserError::Failed(format!(
            "{} fallback failed: {e}",
            chosen.as_str()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_primary_engine_maps_to_its_provider() {
        // The skip check is only correct if this mapping is total over the real engines -
        // a missing arm would silently re-run the CLI that just failed.
        assert_eq!(provider_of(Source::Claude), Some(LlmProvider::Claude));
        assert_eq!(provider_of(Source::Codex), Some(LlmProvider::Codex));
        assert_eq!(provider_of(Source::Copilot), Some(LlmProvider::Copilot));
        assert_eq!(provider_of(Source::CursorAgent), Some(LlmProvider::Cursor));
    }

    #[test]
    fn non_engine_sources_have_no_provider() {
        // `None` here means "never skip", which is the safe direction: these are not
        // engines that could have just failed.
        assert_eq!(provider_of(Source::None), None);
        assert_eq!(provider_of(Source::Mlx), None);
        assert_eq!(provider_of(Source::Fallback(LlmProvider::Claude)), None);
    }
}
