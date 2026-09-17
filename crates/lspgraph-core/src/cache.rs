//! Cross-session cache. Never authoritative: a cold cache costs time only.
//!
//! # Not yet wired up
//!
//! **Nothing outside this module calls [`load`], [`save`] or [`stale_files`]
//! today, and nothing ever inserts into [`CacheFile::file_hashes`].** The
//! round-trip works and is tested, but `Engine` always starts from an empty
//! [`CallGraph`], so spec 5.4's persistence is *not* delivered yet. Treat
//! this module as a tested building block, not as a live feature; in
//! particular `stale_files` currently always returns an empty vector, because
//! the map it reads is always empty.
//!
//! A future consumer — realistically the TUI, which is the first thing with a
//! reason to cache — must do two things, and the second is not optional:
//!
//! 1. **Populate `file_hashes` when caching.** One entry per file the graph
//!    has nodes in, keyed by absolute path, valued by [`hash_file`]. Without
//!    it, invalidation cannot notice that anything changed.
//! 2. **Persist (or rebuild) `Engine`'s `NodeId -> CallHierarchyItem` map.**
//!    `CallGraph` stores node identity, not the `CallHierarchyItem` that the
//!    server needs to answer `callHierarchy/incomingCalls`. `Engine::expand`
//!    returns an *empty* expansion for a node it has no item for, and an
//!    empty expansion is indistinguishable from "this function calls
//!    nothing". Restoring a graph without also restoring the items would
//!    therefore turn a cache hit into a wrong answer — a correctness bug, not
//!    a performance one. Either serialise the item map alongside the graph,
//!    or re-resolve a restored node through `prepareCallHierarchy` on first
//!    expansion.
//!
//! One more thing to fix at wiring time: [`cache_dir_for`] keys the cache
//! directory on the first 64 bits of a SHA-256 of the repository path, and
//! [`CacheFile`] stores no repository path to cross-check against. A
//! collision would hand one repository another repository's graph, which
//! breaches spec 5.4's "a cold cache only costs time, never correctness".
//! Store the repository path in `CacheFile` and reject a load whose path does
//! not match.

use crate::error::Result;
use crate::graph::CallGraph;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct CacheFile {
    pub schema: u32,
    /// Absolute file path -> content hash at the time of caching.
    pub file_hashes: BTreeMap<String, String>,
    pub graph: CallGraph,
}

impl CacheFile {
    pub fn new(graph: CallGraph) -> CacheFile {
        CacheFile {
            schema: SCHEMA_VERSION,
            file_hashes: BTreeMap::new(),
            graph,
        }
    }
}

fn hash_str(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Spec 5.4: platform cache dir, keyed by a hash of the repository path.
pub fn cache_dir_for(repo: &Path) -> PathBuf {
    let key = hash_str(&repo.to_string_lossy());
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    base.join("lspgraph").join(&key[..16])
}

pub fn hash_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}

/// Load a cache, or nothing. A schema mismatch discards rather than migrates.
pub fn load(repo: &Path) -> Option<CacheFile> {
    let path = cache_dir_for(repo).join("graph.json");
    let bytes = std::fs::read(path).ok()?;
    let cache: CacheFile = serde_json::from_slice(&bytes).ok()?;
    // Spec 5.4: discard, never migrate.
    if cache.schema != SCHEMA_VERSION {
        return None;
    }
    Some(cache)
}

pub fn save(repo: &Path, cache: &CacheFile) -> Result<()> {
    let dir = cache_dir_for(repo);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec(cache)?;
    std::fs::write(dir.join("graph.json"), json)?;
    Ok(())
}

/// Cached files whose content hash no longer matches what is on disk.
///
/// The returned strings are NOT filesystem paths: each is the file's URI in
/// the same form as `NodeId.uri` (built via `crate::server::path_to_uri`),
/// so the result can be passed directly, with no conversion, to
/// `Engine::invalidate_file` / `CallGraph::remove_file`.
pub fn stale_files(cache: &CacheFile, _repo: &Path) -> Vec<String> {
    cache
        .file_hashes
        .iter()
        .filter(|(path, cached)| match hash_file(Path::new(path)) {
            Ok(current) => &current != *cached,
            Err(_) => true, // unreadable or deleted counts as stale
        })
        .map(|(path, _)| crate::server::path_to_uri(Path::new(path)).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Expansion, Node, NodeId, NodeState};

    fn id(name: &str, line: u32) -> NodeId {
        NodeId {
            uri: "file:///a.rs".into(),
            line,
            character: 3,
            name: name.into(),
        }
    }

    fn node(name: &str, line: u32) -> Node {
        Node {
            id: id(name, line),
            kind_name: "Function".into(),
            detail: None,
            state: NodeState::Unexpanded,
        }
    }

    #[test]
    fn cache_dir_is_stable_and_repo_specific() {
        let a = cache_dir_for(Path::new("/home/u/proj-a"));
        let b = cache_dir_for(Path::new("/home/u/proj-b"));
        assert_ne!(a, b);
        assert_eq!(a, cache_dir_for(Path::new("/home/u/proj-a")));
        assert!(a.to_string_lossy().contains("lspgraph"));
    }

    #[test]
    fn round_trips_through_disk() {
        let repo =
            std::env::temp_dir().join(format!("lspgraph-t-{}-round-trip", std::process::id()));

        let mut graph = CallGraph::new();
        graph.upsert(node("f", 1));
        let exp = Expansion {
            callers: vec![id("a", 5)],
            callees: vec![id("b", 9)],
        };
        graph.record_expansion(&id("f", 1), &exp);

        let mut cache = CacheFile::new(graph);
        cache
            .file_hashes
            .insert("/some/file.rs".to_string(), "deadbeef".to_string());

        save(&repo, &cache).unwrap();
        let back = load(&repo).expect("should load");

        assert_eq!(back.schema, SCHEMA_VERSION);
        assert_eq!(
            back.file_hashes.get("/some/file.rs").map(String::as_str),
            Some("deadbeef")
        );

        let restored_node = back
            .graph
            .get(&id("f", 1))
            .expect("node survives round trip");
        assert_eq!(restored_node.state, NodeState::Expanded);
        assert_eq!(back.graph.callers_of(&id("f", 1)).unwrap(), &[id("a", 5)]);
        assert_eq!(back.graph.callees_of(&id("f", 1)).unwrap(), &[id("b", 9)]);

        std::fs::remove_dir_all(cache_dir_for(&repo)).ok();
    }

    #[test]
    fn discards_a_mismatched_schema() {
        let repo = std::env::temp_dir().join(format!("lspgraph-s-{}-mismatch", std::process::id()));
        let dir = cache_dir_for(&repo);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("graph.json"),
            br#"{"schema":9999,"file_hashes":{},"graph":{"nodes":[],"callers":[],"callees":[]}}"#,
        )
        .unwrap();
        assert!(load(&repo).is_none(), "mismatched schema must be discarded");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_cache_is_not_an_error() {
        assert!(load(Path::new("/nonexistent/repo/path/xyz")).is_none());
    }

    #[test]
    fn detects_a_changed_file() {
        let f = std::env::temp_dir().join(format!("lspgraph-f-{}-changed.txt", std::process::id()));
        std::fs::write(&f, "one").unwrap();
        let mut cache = CacheFile::new(CallGraph::new());
        cache
            .file_hashes
            .insert(f.to_string_lossy().to_string(), hash_file(&f).unwrap());
        assert!(stale_files(&cache, Path::new("/")).is_empty());

        std::fs::write(&f, "two").unwrap();
        let stale = stale_files(&cache, Path::new("/"));
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0], crate::server::path_to_uri(&f).to_string());
        std::fs::remove_file(&f).ok();
    }

    #[test]
    fn a_deleted_file_counts_as_stale() {
        // Must be absolute but must not exist. A hardcoded POSIX literal
        // would not be absolute on Windows, so build it from the platform's
        // own notion of an absolute directory instead.
        let path = std::env::temp_dir()
            .join(format!(
                "definitely-not-here-lspgraph-cache-test-{}.rs",
                std::process::id()
            ))
            .to_str()
            .unwrap()
            .to_string();
        let mut cache = CacheFile::new(CallGraph::new());
        cache.file_hashes.insert(path.clone(), "abc".to_string());
        let stale = stale_files(&cache, Path::new("/"));
        assert_eq!(stale.len(), 1);
        assert_eq!(
            stale[0],
            crate::server::path_to_uri(Path::new(&path)).to_string()
        );
    }

    #[test]
    fn stale_files_output_can_invalidate_graph_nodes() {
        let f =
            std::env::temp_dir().join(format!("lspgraph-inv-{}-invalidate.rs", std::process::id()));
        std::fs::write(&f, "one").unwrap();
        let uri = crate::server::path_to_uri(&f).to_string();

        let node_id = NodeId {
            uri: uri.clone(),
            line: 1,
            character: 3,
            name: "f".into(),
        };
        let mut graph = CallGraph::new();
        graph.upsert(Node {
            id: node_id.clone(),
            kind_name: "Function".into(),
            detail: None,
            state: NodeState::Unexpanded,
        });
        graph.record_expansion(
            &node_id,
            &Expansion {
                callers: vec![],
                callees: vec![],
            },
        );
        assert_eq!(graph.len(), 1);

        let mut cache = CacheFile::new(graph);
        cache
            .file_hashes
            .insert(f.to_string_lossy().to_string(), hash_file(&f).unwrap());

        std::fs::write(&f, "two").unwrap(); // make it stale

        let stale = stale_files(&cache, Path::new("/"));
        assert_eq!(stale.len(), 1);

        // No conversion: pass the stale_files output straight into
        // CallGraph::remove_file, exactly as a real caller would.
        cache.graph.remove_file(&stale[0]);

        assert!(cache.graph.nodes_in_file(&uri).is_empty());
        assert_eq!(cache.graph.len(), 0);

        std::fs::remove_file(&f).ok();
    }
}
