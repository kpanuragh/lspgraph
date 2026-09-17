//! Integration tests against real language servers.
//!
//! Each test skips when its server is absent, so `cargo test` stays green on
//! a machine with no language servers installed. There is no CI pipeline yet,
//! so these are what "validated" means for the three supported servers.
//!
//! `servers.toml` at the repo root keeps bare command names (`rust-analyzer`,
//! `vtsls --stdio`, `basedpyright-langserver --stdio`), which is correct for
//! distribution. On a machine where a server lives at an absolute,
//! non-PATH location (e.g. installed via an editor plugin manager), set
//! `LSPGRAPH_SERVERS_TOML` to point at a local override file instead of
//! editing the committed one.

use lspgraph_core::config::{Config, ServerConfig};
use lspgraph_core::engine::Engine;
use lspgraph_core::graph::NodeState;
use lspgraph_core::readiness::{wait_until_ready, ReadinessConfig};
use lspgraph_core::server::LanguageServer;
use lspgraph_core::symbols::{collect_named_callables, NamedCallable};
use std::path::{Path, PathBuf};

fn servers_toml() -> Config {
    let path = std::env::var("LSPGRAPH_SERVERS_TOML")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../servers.toml"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    Config::from_toml(&text).expect("valid servers.toml")
}

/// Is this server's program actually runnable on this machine?
///
/// The program comes from the resolved config, not a hardcoded bare name:
/// an override (`LSPGRAPH_SERVERS_TOML`) may point at an absolute path
/// outside `PATH` (e.g. installed via an editor's plugin manager), and a
/// naive `command -v <bare name>` would falsely report it missing.
fn have(cfg: &ServerConfig) -> bool {
    let (prog, _args) = cfg.command();
    if prog.contains('/') {
        std::fs::metadata(&prog)
            .map(|m| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    m.is_file() && m.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    m.is_file()
                }
            })
            .unwrap_or(false)
    } else {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {prog}"))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name)
}

fn source_files(root: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p.ends_with("node_modules") && !p.ends_with("target") {
                    stack.push(p);
                }
            } else if p.extension().and_then(|s| s.to_str()) == Some(ext) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Bring a server up, enumerate, and expand `root` -> `middle` -> `leaf`.
fn assert_three_level_chain(lang: &str, ext: &str, fixture_dir: &str) {
    let cfg = servers_toml();
    let server_cfg = &cfg.servers[lang];
    if !have(server_cfg) {
        eprintln!("skipping {lang}: {} not installed", server_cfg.command().0);
        return;
    }
    let root_dir = fixture(fixture_dir);
    let files = source_files(&root_dir, ext);
    assert!(!files.is_empty(), "no {ext} fixture files");

    let server = LanguageServer::start(lang, server_cfg, &root_dir).expect("server starts");
    // Built from the server table, so a configured `ready_timeout_secs` is
    // genuinely honoured here rather than being parsed and thrown away.
    wait_until_ready(
        &server,
        &files,
        lang,
        &ReadinessConfig::from_server_config(server_cfg),
    )
    .expect("server becomes ready");

    let mut candidates: Vec<NamedCallable> = Vec::new();
    for f in &files {
        server.open(f, lang).ok();
        if let Ok(syms) = server.document_symbols(f) {
            collect_named_callables(
                &syms,
                &lspgraph_core::server::path_to_uri(f),
                &mut candidates,
            );
        }
    }

    // Anonymous callbacks must never survive enumeration.
    //
    // The check targets what actually marks a synthesized name — the word
    // `callback` and empty call-parens, as in `expect() callback` — rather
    // than any parenthesis. gopls legitimately names a Go method
    // `(*counter).bump`, which carries parens but is a real, findable method.
    for c in &candidates {
        assert!(
            !c.name.contains("callback") && !c.name.contains("()"),
            "anonymous callback leaked into enumeration: {}",
            c.name
        );
    }

    let middle = candidates
        .iter()
        .find(|c| c.name == "middle")
        .expect("fixture defines middle")
        .clone();

    let mut engine = Engine::new(server);
    let id = engine.seed(&middle).expect("seed ok").expect("middle resolves");
    let exp = engine.expand(&id).expect("expand ok");

    assert!(
        exp.callers.iter().any(|c| c.name == "root"),
        "middle should be called by root, got {:?}",
        exp.callers
    );
    assert!(
        exp.callees.iter().any(|c| c.name == "leaf"),
        "middle should call leaf, got {:?}",
        exp.callees
    );

    // Memoization: a second expand must not hit the server again.
    let before = engine.expansions_performed();
    let again = engine.expand(&id).expect("second expand ok");
    assert_eq!(engine.expansions_performed(), before, "second expand must be cached");
    assert_eq!(again, exp);

    assert_eq!(engine.graph().get(&id).unwrap().state, NodeState::Expanded);

    // Not optional: without this the server process outlives the test and is
    // reaped only because the test binary exits.
    engine.shutdown().expect("server shuts down");
}

#[test]
fn rust_call_chain() {
    assert_three_level_chain("rust", "rs", "rust-fixture");
}

#[test]
fn typescript_call_chain() {
    assert_three_level_chain("typescript", "ts", "ts-fixture");
}

#[test]
fn python_call_chain() {
    assert_three_level_chain("python", "py", "py-fixture");
}

#[test]
fn go_call_chain() {
    assert_three_level_chain("go", "go", "go-fixture");
}

/// clangd needs a compile database to analyse anything, the same way
/// rust-analyzer needs a loadable Cargo manifest. Its `directory` field must be
/// absolute, so committing one would bake in whoever generated it — this writes
/// it for the machine actually running the test.
fn write_compile_commands(fixture_dir: &Path) {
    let json = format!(
        "[{{\n  \"directory\": {:?},\n  \"command\": \"cc -c src/main.c -o /dev/null\",\n  \"file\": \"src/main.c\"\n}}]\n",
        fixture_dir.to_string_lossy()
    );
    std::fs::write(fixture_dir.join("compile_commands.json"), json)
        .expect("write compile_commands.json");
}

#[test]
fn c_call_chain() {
    write_compile_commands(&fixture("c-fixture"));
    assert_three_level_chain("c", "c", "c-fixture");
}

#[test]
fn typescript_unresolvable_symbols_are_recorded_not_dropped() {
    let cfg = servers_toml();
    let server_cfg = &cfg.servers["typescript"];
    if !have(server_cfg) {
        eprintln!("skipping: {} not installed", server_cfg.command().0);
        return;
    }
    let root_dir = fixture("ts-fixture");
    let files = source_files(&root_dir, "ts");
    let server = LanguageServer::start("typescript", server_cfg, &root_dir).unwrap();
    wait_until_ready(
        &server,
        &files,
        "typescript",
        &ReadinessConfig::from_server_config(server_cfg),
    )
    .unwrap();

    let mut candidates = Vec::new();
    for f in &files {
        server.open(f, "typescript").ok();
        if let Ok(syms) = server.document_symbols(f) {
            collect_named_callables(
                &syms,
                &lspgraph_core::server::path_to_uri(f),
                &mut candidates,
            );
        }
    }

    let candidate_count = candidates.len();
    let mut engine = Engine::new(server);
    let mut unresolved = 0;
    let mut resolved = 0;
    for c in &candidates {
        match engine.seed(c).unwrap() {
            Some(_) => resolved += 1,
            None => unresolved += 1,
        }
    }

    // Spec 5.5: whatever failed to resolve is still present in the graph —
    // it must not be silently dropped. `seed` inserts exactly one graph node
    // per candidate: an `Unresolved` node when resolution fails, or (via
    // `intern`) the resolved call-hierarchy item's own node when it
    // succeeds. `intern` also inserts nodes for callers/callees discovered
    // during `expand`, but nothing here calls `expand`, so no such extra
    // nodes exist yet and the graph must contain exactly one node per
    // candidate seeded — neither fewer (a drop) nor more (a double-count).
    assert_eq!(
        engine.graph().len(),
        candidate_count,
        "every seeded candidate — resolved or not — must land in the graph exactly once"
    );
    assert!(unresolved > 0, "fixture's overload/arrow cases should be unresolved");
    assert_eq!(resolved + unresolved, candidate_count);

    engine.shutdown().expect("server shuts down");
}
