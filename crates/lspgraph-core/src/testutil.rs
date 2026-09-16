//! Test-only helpers shared by several modules' unit tests.
//!
//! Some behaviour can only be checked against a process: that a dropped
//! `LanguageServer` really kills its child, or that a specific JSON-RPC error
//! reaches `Engine::seed`. A `/bin/sh` script standing in for a language
//! server is enough for both, and is far cheaper and more deterministic than
//! a real server.
//!
//! Each script gets a name unique to this process AND to the individual
//! script, because the leak checks use `pgrep -f <name>`: a shared name would
//! let one test see another test's process (tests run concurrently in one
//! process) and either hang or fail for the wrong reason.

use crate::config::ServerConfig;
use crate::server::LanguageServer;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static SCRIPTS_CREATED: AtomicUsize = AtomicUsize::new(0);

/// A uniquely named executable `/bin/sh` script in the temp directory,
/// deleted from disk when dropped.
pub struct FakeServerScript {
    name: String,
    path: PathBuf,
}

impl FakeServerScript {
    /// Write `body` as the contents of a `/bin/sh` script (the shebang is
    /// added for you). `tag` only makes the filename readable in `ps` output.
    pub fn new(tag: &str, body: &str) -> FakeServerScript {
        use std::os::unix::fs::PermissionsExt;

        let n = SCRIPTS_CREATED.fetch_add(1, Ordering::SeqCst);
        let name = format!("lspgraph-fake-{tag}-{}-{n}.sh", std::process::id());
        let path = std::env::temp_dir().join(&name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write fake server script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake server script");
        FakeServerScript { name, path }
    }

    /// The unique basename, which doubles as a `pgrep -f` pattern.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn config(&self) -> ServerConfig {
        ServerConfig {
            cmd: self.path.display().to_string(),
            extensions: vec![],
            ready_timeout_secs: 300,
            concurrency: 1,
        }
    }

    /// Start a `LanguageServer` against this script.
    ///
    /// Retries on `ETXTBSY`. Several of these tests run concurrently in one
    /// process, and a sibling test's fork can still be holding a write handle
    /// to this freshly written script at the moment we exec it — Linux then
    /// refuses the exec with "text file busy". That is a property of writing
    /// executables from a multi-threaded test harness, not of
    /// `LanguageServer::start`, so it is absorbed here rather than papered
    /// over in the tests.
    pub fn start(&self) -> crate::error::Result<LanguageServer> {
        const ETXTBSY: i32 = 26;
        let mut last = None;
        for _ in 0..40 {
            match LanguageServer::start("fake", &self.config(), &std::env::temp_dir()) {
                Err(crate::error::Error::Io(e)) if e.raw_os_error() == Some(ETXTBSY) => {
                    last = Some(crate::error::Error::Io(e));
                    std::thread::sleep(Duration::from_millis(50));
                }
                other => return other,
            }
        }
        Err(last.expect("loop ran at least once"))
    }

    /// Is a process running this script right now?
    pub fn is_running(&self) -> bool {
        Command::new("pgrep")
            .arg("-f")
            .arg(&self.name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// True once no process matching this script's unique name is running.
    ///
    /// Polls for a short grace period rather than asserting immediately, so a
    /// test never races the kill/wait it is checking for.
    pub fn no_process_survives(&self) -> bool {
        for _ in 0..40 {
            if !self.is_running() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }
}

impl Drop for FakeServerScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Shell source that emits `body` as a single LSP frame.
///
/// `body` must contain no `%` (printf format) and no `'` (the quoting used
/// here); every JSON payload these tests send satisfies both.
pub fn sh_send_frame(body: &str) -> String {
    debug_assert!(!body.contains('%') && !body.contains('\''));
    format!("printf 'Content-Length: {}\\r\\n\\r\\n{}'\n", body.len(), body)
}

/// A successful `initialize` result that passes the call-hierarchy gate.
pub const INITIALIZE_OK: &str =
    r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"callHierarchyProvider":true}}}"#;

/// Shell source for a loop that keeps the script alive without ever reading
/// stdin, bounded so a failing test cannot leave a process behind for long.
pub const SH_IDLE: &str = "i=0\nwhile [ $i -lt 120 ]; do sleep 1; i=$((i+1)); done\n";
