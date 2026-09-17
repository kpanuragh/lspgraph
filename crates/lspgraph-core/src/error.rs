use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error("{method}: server reported content modified (transient)")]
    ContentModified { method: String },

    #[error("config: {0}")]
    Config(String),

    #[error("language server {server} does not advertise callHierarchyProvider")]
    NoCallHierarchy { server: String },

    #[error("request {method} timed out after {}ms", timeout.as_millis())]
    Timeout { method: String, timeout: Duration },

    #[error("language server exited")]
    ServerExited,

    #[error("server not ready after {}s; tried {tried} candidate symbols", timeout.as_secs())]
    NotReady { timeout: Duration, tried: usize },

    #[error("no candidate symbols found: {files_attempted} files examined, {files_failed} of them failed to respond")]
    NoCandidates {
        files_attempted: usize,
        files_failed: usize,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
