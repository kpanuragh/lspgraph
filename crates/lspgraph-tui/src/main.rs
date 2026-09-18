//! Terminal interface for exploring a codebase's call graph.

mod app;
mod keys;
mod protocol;
mod ui;
mod worker;

use app::App;
use lspgraph_core::config::Config;
use lspgraph_core::engine::Engine;
use lspgraph_core::readiness::{wait_until_ready, ReadinessConfig};
use lspgraph_core::server::LanguageServer;
use protocol::{Event, Request};
use ratatui::crossterm::event::{self, Event as CEvent};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, TryRecvError};
use std::time::Duration;

fn source_files(root: &Path, exts: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
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
                .map(|x| exts.iter().any(|e| e == x))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Where a user-level `servers.toml` lives, following the platform convention.
/// Deliberately computed from environment variables rather than pulling in a
/// directories crate: the interface's dependencies are ratatui and the engine,
/// and one config path is not worth widening that.
fn user_config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.map(config_path_in)
}

/// Split from `user_config_path` so the layout can be asserted without setting
/// environment variables: tests share a process, and mutating the environment
/// from one test races every other test in the binary.
fn config_path_in(base: PathBuf) -> PathBuf {
    base.join("lspgraph").join("servers.toml")
}

/// The search order, most specific first.
///
/// Looking only in the working directory made an installed binary unusable from
/// anywhere that is not a checkout of this repository -- every invocation had to
/// set LSPGRAPH_SERVERS_TOML by hand.
fn find_servers_toml() -> Result<PathBuf, String> {
    resolve_servers_toml(
        std::env::var_os("LSPGRAPH_SERVERS_TOML").map(PathBuf::from),
        std::env::current_dir()
            .map(|d| d.join("servers.toml"))
            .unwrap_or_else(|_| PathBuf::from("servers.toml")),
        user_config_path(),
    )
}

/// The resolution itself, with every input passed in so it can be tested
/// directly. `exists` is the only thing it touches outside its arguments.
fn resolve_servers_toml(
    override_: Option<PathBuf>,
    cwd: PathBuf,
    user: Option<PathBuf>,
) -> Result<PathBuf, String> {
    if let Some(p) = override_ {
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!(
            "LSPGRAPH_SERVERS_TOML points at {}, which is not a readable file",
            p.display()
        ));
    }

    let mut tried = vec![cwd];
    tried.extend(user);
    for p in &tried {
        if p.is_file() {
            return Ok(p.clone());
        }
    }

    let looked: Vec<String> = tried.iter().map(|p| format!("  {}", p.display())).collect();
    Err(format!(
        "no servers.toml found. Looked in:\n{}\n\nCreate one of those, or set \
         LSPGRAPH_SERVERS_TOML. A minimal file looks like:\n\n  [python]\n  \
         cmd = \"basedpyright-langserver --stdio\"\n  extensions = [\"py\"]",
        looked.join("\n")
    ))
}

fn main() {
    // `fn main() -> Result<..>` reports the error with Debug, which renders a
    // multi-line message as a single line of \n escapes -- unreadable for the
    // "no servers.toml found" text, which is a list.
    if let Err(e) = run() {
        eprintln!("lspgraph: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let lang = args.next().ok_or("usage: lspgraph <language> <root>")?;
    let root =
        PathBuf::from(args.next().ok_or("usage: lspgraph <language> <root>")?).canonicalize()?;

    let cfg_path = find_servers_toml()?;
    let cfg = Config::from_toml(&std::fs::read_to_string(&cfg_path)?)?;
    let server_cfg = cfg.servers.get(&lang).ok_or_else(|| {
        let mut known: Vec<&str> = cfg.servers.keys().map(|s| s.as_str()).collect();
        known.sort();
        format!(
            "no language named {lang:?} in {}. It defines: {}",
            cfg_path.display(),
            known.join(", ")
        )
    })?;
    let files = source_files(&root, &server_cfg.extensions);

    // After the configuration is resolved, so a bad servers.toml still reports
    // itself when output is piped, but before the language server is spawned:
    // waiting out readiness only to fail on a missing terminal would throw away
    // the slowest part of startup.
    if !std::io::stdout().is_terminal() {
        return Err(
            "lspgraph draws a full-screen interface and needs a terminal; \
                    stdout is not one here (piped or redirected?)"
                .into(),
        );
    }

    // Startup happens before the terminal is taken over, so a failure prints
    // plainly instead of being swallowed by an alternate screen.
    eprintln!("starting {lang} server…");
    let server = LanguageServer::start(&lang, server_cfg, &root)?;
    let can_search = server.supports_workspace_symbol();
    eprintln!("waiting for the server to become ready…");
    wait_until_ready(
        &server,
        &files,
        &lang,
        &ReadinessConfig::from_server_config(server_cfg),
    )?;

    let engine = worker::CoreEngine {
        engine: Engine::new(server),
        can_search,
    };
    let (qtx, qrx) = channel::<Request>();
    let (etx, erx) = channel::<Event>();
    let handle = worker::spawn_worker(Box::new(engine), qrx, etx);

    let mut terminal = ratatui::init();
    let mut app = App::new();
    let res = (|| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            terminal.draw(|f| ui::draw(f, &app))?;

            loop {
                match erx.try_recv() {
                    Ok(ev) => {
                        for r in app.on_event(ev) {
                            let _ = qtx.send(r);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        app.should_quit = true;
                        break;
                    }
                }
            }

            if event::poll(Duration::from_millis(50))? {
                if let CEvent::Key(k) = event::read()? {
                    for r in keys::handle_key(&mut app, k) {
                        let _ = qtx.send(r);
                    }
                }
            }

            if app.should_quit {
                return Ok(());
            }
        }
    })();

    ratatui::restore();
    let _ = qtx.send(Request::Shutdown);
    drop(qtx);
    let _ = handle.join();
    // A startup or draw failure still surfaces first and on its own terms.
    res?;
    // A `Fatal` sets both `error` and `should_quit` in the same drain, so the
    // loop returns before it can be drawn and `restore()` then wipes the
    // alternate screen. Without this the session would vanish with no
    // explanation and an exit status of 0.
    if let Some(e) = app.error() {
        eprintln!("lspgraph: {e}");
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_that_does_not_exist_is_named_rather_than_silently_ignored() {
        let e = resolve_servers_toml(
            Some(PathBuf::from("/definitely/not/here/servers.toml")),
            PathBuf::from("servers.toml"),
            None,
        )
        .unwrap_err();
        assert!(e.contains("/definitely/not/here/servers.toml"), "{e}");
    }

    #[test]
    fn the_failure_lists_every_path_it_looked_in() {
        let e = resolve_servers_toml(
            None,
            PathBuf::from("/no/such/cwd/servers.toml"),
            Some(PathBuf::from("/no/such/home/.config/lspgraph/servers.toml")),
        )
        .unwrap_err();
        assert!(e.contains("/no/such/cwd/servers.toml"), "{e}");
        assert!(
            e.contains("/no/such/home/.config/lspgraph/servers.toml"),
            "the user-level path must be named, or the user cannot know where to \
             put the file:\n{e}"
        );
    }

    #[test]
    fn the_user_level_file_is_used_when_the_working_directory_has_none() {
        let dir = std::env::temp_dir().join(format!("lspgraph-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let user = dir.join("servers.toml");
        std::fs::write(&user, "[python]\ncmd = \"x\"\nextensions = [\"py\"]\n").unwrap();

        let got = resolve_servers_toml(
            None,
            PathBuf::from("/no/such/cwd/servers.toml"),
            Some(user.clone()),
        )
        .unwrap();
        assert_eq!(got, user);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_user_config_path_is_under_the_platform_config_directory() {
        let p = config_path_in(PathBuf::from("/xdg"));
        assert_eq!(p, PathBuf::from("/xdg/lspgraph/servers.toml"));
    }
}
