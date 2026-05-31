// meridian — normalises screenpipe activity into structured app sessions
//
// The single global LLM gate.
//
// Every call to the local MLX model server — Stage 1 classify
// (`/classify_sessions`), Stage 2/3 summarise fallback (`/summarise`), and the
// Stage 4 pm-worklog synth (`/synthesise_worklog`) — must hold this one permit
// for the duration of the HTTP request. The MLX server hosts a single model on a
// single GPU; serialising at the client guarantees **exactly one model call is
// ever in flight**, so the stages can never contend (the classifier and the
// worklog synthesiser sharing the GPU was the concrete risk this closes).
//
// One permit, process-global. Acquire it *per call* — never hold it across a
// whole batch — so the stages interleave fairly instead of one starving the
// others. The guard releases on drop, so a timed-out or failed request frees the
// gate immediately. External Claude/Codex subprocess calls (a different resource,
// the user's subscription, not the local GPU) are intentionally NOT gated here.

use std::sync::OnceLock;

use tokio::sync::{Semaphore, SemaphorePermit};

/// Process-global single-permit semaphore. Initialised on first use.
static GATE: OnceLock<Semaphore> = OnceLock::new();

fn gate() -> &'static Semaphore {
    GATE.get_or_init(|| Semaphore::new(1))
}

/// Acquire the single global LLM permit, awaiting if another local-MLX call is in
/// flight. Hold the returned guard only for the duration of one request, then let
/// it drop. Never `.await` another gated LLM call while holding the guard — with a
/// single permit that would self-deadlock.
pub async fn acquire() -> SemaphorePermit<'static> {
    gate()
        .acquire()
        .await
        .expect("llm gate semaphore is never closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// With a single permit, two tasks can never be inside the guarded section at
    /// the same time — `in_flight` must never exceed 1.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn gate_serialises_concurrent_callers() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let in_flight = in_flight.clone();
            let max_seen = max_seen.clone();
            handles.push(tokio::spawn(async move {
                let _permit = acquire().await;
                let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "gate let >1 caller through"
        );
    }

    /// The permit is released on drop, so sequential acquires always succeed.
    #[tokio::test]
    async fn permit_releases_on_drop() {
        {
            let _p = acquire().await;
        }
        // If the first permit had leaked, this would hang; the test timeout guards.
        let _p2 = acquire().await;
    }
}
