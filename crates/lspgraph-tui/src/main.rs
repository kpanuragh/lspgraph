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
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, TryRecvError};
use std::time::Duration;

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let lang = args.next().ok_or("usage: lspgraph <language> <root>")?;
    let root = PathBuf::from(args.next().ok_or("usage: lspgraph <language> <root>")?)
        .canonicalize()?;

    let cfg_path = std::env::var("LSPGRAPH_SERVERS_TOML")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("servers.toml"));
    let cfg = Config::from_toml(&std::fs::read_to_string(&cfg_path)?)?;
    let server_cfg = cfg.servers.get(&lang).ok_or("unknown language")?;
    let files = source_files(&root, &server_cfg.extensions);

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

    let engine = worker::CoreEngine { engine: Engine::new(server), can_search };
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
