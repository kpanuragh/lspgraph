//! End-to-end driver: crawl a repository's call graph and print a summary.
//!
//! Usage: cargo run --example crawl -- <language> <root>

use lspgraph_core::config::Config;
use lspgraph_core::engine::Engine;
use lspgraph_core::graph::NodeState;
use lspgraph_core::readiness::{wait_until_ready, ReadinessConfig};
use lspgraph_core::server::{path_to_uri, LanguageServer};
use lspgraph_core::symbols::{collect_named_callables, NamedCallable};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn source_files(root: &Path, exts: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if p.is_dir() {
                if !matches!(name, ".git" | "node_modules" | "target" | "vendor") {
                    stack.push(p);
                }
            } else if p
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| exts.iter().any(|x| x == e))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let lang = args.next().ok_or("usage: crawl <language> <root>")?;
    let root = PathBuf::from(args.next().ok_or("usage: crawl <language> <root>")?).canonicalize()?;

    let servers_toml_path = std::env::var("LSPGRAPH_SERVERS_TOML")
        .unwrap_or_else(|_| "servers.toml".to_string());
    let cfg = Config::from_toml(&std::fs::read_to_string(&servers_toml_path)?)?;
    let server_cfg = cfg.servers.get(&lang).ok_or("unknown language")?;
    let files = source_files(&root, &server_cfg.extensions);
    println!("{} files", files.len());

    let t0 = Instant::now();
    let server = LanguageServer::start(&lang, server_cfg, &root)?;
    wait_until_ready(&server, &files, &lang, &ReadinessConfig::default())?;
    println!("ready in {:.1}s", t0.elapsed().as_secs_f64());

    let t1 = Instant::now();
    let mut candidates: Vec<NamedCallable> = Vec::new();
    for f in &files {
        server.open(f, &lang).ok();
        if let Ok(syms) = server.document_symbols(f) {
            collect_named_callables(&syms, &path_to_uri(f), &mut candidates);
        }
    }
    println!(
        "{} named callables in {:.1}s",
        candidates.len(),
        t1.elapsed().as_secs_f64()
    );

    let t2 = Instant::now();
    let mut engine = Engine::new(server);
    let (mut resolved, mut unresolved) = (0usize, 0usize);
    for c in candidates.iter().take(120) {
        match engine.seed(c)? {
            Some(id) => {
                engine.expand(&id)?;
                resolved += 1;
            }
            None => unresolved += 1,
        }
    }
    let elapsed = t2.elapsed().as_secs_f64();
    println!(
        "expanded {resolved}, unresolved {unresolved} in {elapsed:.1}s ({:.1} sym/s)",
        resolved as f64 / elapsed.max(0.001)
    );

    let leaves = engine
        .graph()
        .nodes_in_file(&path_to_uri(&files[0]).to_string())
        .into_iter()
        .filter(|id| {
            matches!(
                engine.graph().get(id).map(|n| &n.state),
                Some(NodeState::Unresolved(_))
            )
        })
        .count();
    println!("unresolved nodes recorded in first file: {leaves}");
    Ok(())
}
