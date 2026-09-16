//! Decide when a language server is genuinely ready.
//!
//! There is no portable readiness signal. rust-analyzer's cachePriming token
//! is proprietary; other servers differ. Quiet-period heuristics are wrong:
//! rust-analyzer goes silent for ~17s during dependency fetch. The only
//! portable answer is to poll a real semantic request across MANY candidates
//! and accept the first that answers.

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
        Self { max_files: 40, per_file: 3, timeout: Duration::from_secs(300) }
    }
}

/// Sample up to `max` items spread evenly across the slice.
///
/// Taking the first N alphabetically is a trap: it can land entirely inside
/// one directory (a benchmark tree with unresolvable imports, say), which
/// makes a healthy server look dead.
pub fn even_stride<T>(items: &[T], max: usize) -> Vec<&T> {
    if items.is_empty() || max == 0 {
        return Vec::new();
    }
    if items.len() <= max {
        return items.iter().collect();
    }
    let stride = items.len() / max;
    items.iter().step_by(stride.max(1)).take(max).collect()
}

pub fn wait_until_ready(
    server: &LanguageServer,
    files: &[PathBuf],
    language_id: &str,
    cfg: &ReadinessConfig,
) -> Result<()> {
    let mut candidates: Vec<NamedCallable> = Vec::new();
    for path in even_stride(files, cfg.max_files) {
        if server.open(path, language_id).is_err() {
            continue;
        }
        let Ok(syms) = server.document_symbols(path) else {
            continue;
        };
        let mut found = Vec::new();
        collect_named_callables(&syms, &crate::server::path_to_uri(path), &mut found);
        found.truncate(cfg.per_file);
        candidates.extend(found);
    }

    if candidates.is_empty() {
        return Err(Error::NotReady { timeout: Duration::ZERO, tried: 0 });
    }

    let start = Instant::now();
    while start.elapsed() < cfg.timeout {
        for c in &candidates {
            let path = c.uri.to_file_path().unwrap_or_default();
            if let Ok(items) = server.prepare_call_hierarchy(&path, c.position) {
                if !items.is_empty() {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    Err(Error::NotReady { timeout: cfg.timeout, tried: candidates.len() })
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
}
