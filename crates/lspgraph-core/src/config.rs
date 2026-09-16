//! Server configuration. Adding a language means adding a table, not code.

use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

fn default_ready_timeout() -> u64 {
    300
}

fn default_concurrency() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Full command line, e.g. `"vtsls --stdio"`.
    pub cmd: String,
    /// File extensions without a leading dot.
    pub extensions: Vec<String>,
    /// How long to wait for the server to become genuinely ready, in seconds
    /// (spec 5.2; default 300). Read via
    /// [`crate::readiness::ReadinessConfig::from_server_config`].
    #[serde(default = "default_ready_timeout")]
    pub ready_timeout_secs: u64,
    /// RESERVED — parsed and validated, but **not consumed by anything yet**.
    ///
    /// Spec 5.6 ships a configurable concurrency limit defaulting to serial,
    /// with the real number to be set by measurement rather than assumption.
    /// The measurement has not been done and the pipelining is not written,
    /// so every crawl today is serial no matter what this says. The key is
    /// accepted now so that configs written against it keep parsing, and is
    /// documented here (and in the README) so nobody sets it expecting a
    /// speedup.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
}

impl ServerConfig {
    /// Split `cmd` into program and arguments.
    pub fn command(&self) -> (String, Vec<String>) {
        let mut parts = self.cmd.split_whitespace().map(str::to_string);
        let prog = parts.next().unwrap_or_default();
        (prog, parts.collect())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub struct Config {
    pub servers: BTreeMap<String, ServerConfig>,
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Config> {
        toml::from_str(text).map_err(|e| Error::Config(e.to_string()))
    }

    /// Resolve an extension (no leading dot) to a language id and its server.
    pub fn for_extension(&self, ext: &str) -> Option<(&str, &ServerConfig)> {
        self.servers
            .iter()
            .find(|(_, s)| s.extensions.iter().any(|e| e == ext))
            .map(|(lang, s)| (lang.as_str(), s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[rust]
cmd = "rust-analyzer"
extensions = ["rs"]

[typescript]
cmd = "vtsls --stdio"
extensions = ["ts", "tsx"]
concurrency = 4
"#;

    #[test]
    fn parses_servers() {
        let c = Config::from_toml(SAMPLE).unwrap();
        assert_eq!(c.servers.len(), 2);
        assert_eq!(c.servers["rust"].cmd, "rust-analyzer");
    }

    #[test]
    fn applies_documented_defaults() {
        let c = Config::from_toml(SAMPLE).unwrap();
        // Spec 5.2: 300s readiness. Spec 5.6: serial by default.
        assert_eq!(c.servers["rust"].ready_timeout_secs, 300);
        assert_eq!(c.servers["rust"].concurrency, 1);
        assert_eq!(c.servers["typescript"].concurrency, 4);
    }

    #[test]
    fn splits_command_into_program_and_args() {
        let c = Config::from_toml(SAMPLE).unwrap();
        let (prog, args) = c.servers["typescript"].command();
        assert_eq!(prog, "vtsls");
        assert_eq!(args, vec!["--stdio".to_string()]);
    }

    #[test]
    fn resolves_extension_to_language() {
        let c = Config::from_toml(SAMPLE).unwrap();
        assert_eq!(c.for_extension("tsx").unwrap().0, "typescript");
        assert_eq!(c.for_extension("rs").unwrap().0, "rust");
        assert!(c.for_extension("go").is_none());
    }

    #[test]
    fn reports_malformed_toml() {
        assert!(matches!(Config::from_toml("[rust"), Err(Error::Config(_))));
    }
}
