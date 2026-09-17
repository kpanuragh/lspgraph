//! Decide when a language server is genuinely ready.
//!
//! There is no portable readiness signal. rust-analyzer's cachePriming token
//! is proprietary; other servers differ. Quiet-period heuristics are wrong:
//! rust-analyzer goes silent for ~17s during dependency fetch. The only
//! portable answer is to poll a real semantic request across MANY candidates
//! and accept the first that answers.

use crate::config::ServerConfig;
use crate::error::{Error, Result};
use crate::server::LanguageServer;
use crate::symbols::{collect_named_callables, NamedCallable};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ReadinessConfig {
    pub max_files: usize,
    pub per_file: usize,
    pub timeout: Duration,
}

impl Default for ReadinessConfig {
    fn default() -> Self {
        // Spec 5.2.
        Self {
            max_files: 40,
            per_file: 3,
            timeout: Duration::from_secs(300),
        }
    }
}

impl ReadinessConfig {
    /// Honour a server table's `ready_timeout_secs`.
    ///
    /// Spec 5.2 requires the readiness timeout to be configurable. Callers
    /// that build a `ReadinessConfig::default()` from a `ServerConfig` they
    /// already hold silently ignore whatever the user wrote, which is worse
    /// than having no knob at all. The sampling bounds keep their defaults;
    /// only the timeout is configurable today.
    pub fn from_server_config(cfg: &ServerConfig) -> ReadinessConfig {
        ReadinessConfig {
            timeout: Duration::from_secs(cfg.ready_timeout_secs),
            ..ReadinessConfig::default()
        }
    }
}

/// Sample up to `max` items spread evenly across the slice.
///
/// Taking the first N alphabetically is a trap: it can land entirely inside
/// one directory (a benchmark tree with unresolvable imports, say), which
/// makes a healthy server look dead. That is spec 5.2's 600-second false
/// failure.
///
/// The index is computed as `i * len / max` rather than by stepping with an
/// integer stride. An integer stride collapses to 1 for every `len` between
/// `max` and `2 * max` — a 40-file cap over a 79-file repository would then
/// take files 0..=39, i.e. the first alphabetical half, which is exactly the
/// clustering this function exists to avoid.
pub fn even_stride<T>(items: &[T], max: usize) -> Vec<&T> {
    if items.is_empty() || max == 0 {
        return Vec::new();
    }
    if items.len() <= max {
        return items.iter().collect();
    }
    let len = items.len();
    (0..max).map(|i| &items[i * len / max]).collect()
}

/// Time left before `deadline`, or `None` if it has already passed.
pub fn remaining_budget(deadline: Instant, now: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(now)
        .filter(|d| !d.is_zero())
}

pub fn wait_until_ready(
    server: &LanguageServer,
    files: &[PathBuf],
    language_id: &str,
    cfg: &ReadinessConfig,
) -> Result<()> {
    // One deadline governs both gathering and polling. Single-target polling
    // and quiet-period heuristics were both wrong for the same underlying
    // reason: they let one phase run unbounded while the clock looked away.
    let deadline = Instant::now() + cfg.timeout;

    let mut candidates: Vec<NamedCallable> = Vec::new();
    let mut files_attempted = 0usize;
    let mut files_failed = 0usize;
    for path in even_stride(files, cfg.max_files) {
        if remaining_budget(deadline, Instant::now()).is_none() {
            break;
        }
        files_attempted += 1;
        if server.open(path, language_id).is_err() {
            files_failed += 1;
            continue;
        }
        let Ok(syms) = server.document_symbols(path) else {
            files_failed += 1;
            continue;
        };
        let mut found = Vec::new();
        collect_named_callables(&syms, &crate::server::path_to_uri(path), &mut found);
        found.truncate(cfg.per_file);
        candidates.extend(found);
    }

    if candidates.is_empty() {
        return Err(Error::NoCandidates {
            files_attempted,
            files_failed,
        });
    }

    while remaining_budget(deadline, Instant::now()).is_some() {
        for c in &candidates {
            if remaining_budget(deadline, Instant::now()).is_none() {
                break;
            }
            let path = c.uri.to_file_path().unwrap_or_default();
            // Checking the budget before each candidate (not once per round)
            // is the fix: a round of up to max_files * per_file requests
            // could otherwise blow past the deadline by however long each
            // individual request takes to time out. We deliberately do not
            // plumb a shrinking per-request budget into server.rs's LSP
            // calls for this — that's a cross-task change to save a residual
            // overshoot already bounded by server.rs's own REQUEST_TIMEOUT
            // (60s), i.e. at most one in-flight request past the deadline
            // instead of an entire unbounded round.
            if let Ok(items) = server.prepare_call_hierarchy(&path, c.position) {
                if !items.is_empty() {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    Err(Error::NotReady {
        timeout: cfg.timeout,
        tried: candidates.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_returns_everything_when_under_the_cap() {
        let v = vec![1, 2, 3];
        assert_eq!(even_stride(&v, 10), vec![&1, &2, &3]);
    }

    #[test]
    fn stride_spreads_across_the_whole_slice() {
        let v: Vec<i32> = (0..100).collect();
        let picked = even_stride(&v, 5);
        assert_eq!(picked.len(), 5);
        // Must reach the far end, not cluster at the start.
        assert!(**picked.last().unwrap() >= 80, "clustered: {picked:?}");
        assert_eq!(**picked.first().unwrap(), 0);
    }

    #[test]
    fn stride_spreads_at_a_realistic_repository_ratio() {
        // The ratio that matters in practice, and the one an integer stride
        // gets wrong: max < len < 2 * max. A repository of 79 files under the
        // 40-file cap must not collapse to "the first 40 files".
        let v: Vec<i32> = (0..79).collect();
        let picked = even_stride(&v, 40);
        assert_eq!(picked.len(), 40);
        assert_eq!(**picked.first().unwrap(), 0);
        // The last pick must land in the final quarter of the slice.
        assert!(
            **picked.last().unwrap() >= 79 * 3 / 4,
            "clustered in the first half of the slice: {picked:?}"
        );
        // ...and no file may be sampled twice.
        let mut sorted: Vec<i32> = picked.iter().map(|x| **x).collect();
        sorted.dedup();
        assert_eq!(sorted.len(), 40, "duplicate picks: {picked:?}");
    }

    #[test]
    fn stride_handles_empty_and_zero_max() {
        let empty: Vec<i32> = Vec::new();
        assert!(even_stride(&empty, 5).is_empty());
        let v = vec![1, 2, 3];
        assert!(even_stride(&v, 0).is_empty());
    }

    #[test]
    fn defaults_match_the_spec() {
        let c = ReadinessConfig::default();
        assert_eq!(c.max_files, 40);
        assert_eq!(c.per_file, 3);
        assert_eq!(c.timeout, Duration::from_secs(300));
    }

    #[test]
    fn a_configured_ready_timeout_reaches_the_readiness_config() {
        // A `ready_timeout_secs` that nothing reads is a lie in the config
        // file; spec 5.2 requires it to be honoured.
        let cfg = ServerConfig {
            cmd: "fake".into(),
            extensions: vec!["rs".into()],
            ready_timeout_secs: 42,
            concurrency: 1,
        };
        let rc = ReadinessConfig::from_server_config(&cfg);
        assert_eq!(rc.timeout, Duration::from_secs(42));
        assert_ne!(rc.timeout, ReadinessConfig::default().timeout);
        // Only the timeout is configurable; the sampling bounds are the spec
        // defaults.
        assert_eq!(rc.max_files, ReadinessConfig::default().max_files);
        assert_eq!(rc.per_file, ReadinessConfig::default().per_file);
    }

    #[test]
    fn remaining_budget_reports_time_left_when_deadline_is_ahead() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(10);
        let left = remaining_budget(deadline, now).expect("deadline is ahead");
        assert!(left <= Duration::from_secs(10) && left > Duration::from_secs(9));
    }

    #[test]
    fn remaining_budget_is_none_at_or_past_the_deadline() {
        let now = Instant::now();
        assert!(remaining_budget(now, now).is_none());
        let past_deadline = now - Duration::from_secs(1);
        assert!(remaining_budget(past_deadline, now).is_none());
    }

    #[test]
    fn zero_candidates_reports_no_candidates_not_a_zero_timeout() {
        let err = Error::NoCandidates {
            files_attempted: 5,
            files_failed: 3,
        };
        let msg = err.to_string();
        assert!(
            msg.contains('5') && msg.contains('3'),
            "message should mention both counts: {msg}"
        );
        assert!(
            msg.contains("files examined") && msg.contains("failed to respond"),
            "message should describe the gathering outcome: {msg}"
        );
        assert!(!matches!(err, Error::NotReady { .. }));
    }
}
