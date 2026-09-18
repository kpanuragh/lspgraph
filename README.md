# lspgraph

Explore a codebase's call graph by driving any LSP server's call hierarchy API.

Sourcetrail did this well and was archived in 2021. It maintained a
hand-written indexer per language, which capped it at four languages and made
maintenance unsustainable. lspgraph writes no indexers: it is an LSP client, so
every language with a language server is reachable.

## Status

`lspgraph-core` is the engine, and `lspgraph-tui` is a working terminal
interface built on it: `cargo run -p lspgraph-tui -- <language> <root>`
starts the language server, waits for it to report itself genuinely ready,
then lets you search for a symbol and walk its call graph. See "Try it"
below.

Caching is **not yet wired up**. `lspgraph_core::cache` can serialize and
restore a graph and is tested, but nothing calls it: every run starts cold.
See that module's documentation for what a consumer has to do first.

## Server support

Adding a language means adding a table to `servers.toml`. No code.

| Server | Status |
|--------|--------|
| rust-analyzer | validated by integration tests against the real server |
| vtsls | validated by integration tests against the real server |
| basedpyright | validated by integration tests against the real server |
| gopls | validated by integration tests against the real server |
| clangd | validated by integration tests against the real server, at 23.1.0 — see note below |
| anything else implementing `callHierarchyProvider` | expected to work, untested |

Two of these need something on the machine beyond the server itself. `gopls`
shells out to the Go toolchain, so `go` must be on `PATH` or it will answer
nothing — which surfaces as `NoCandidates`, not as a hang. `clangd` needs a
`compile_commands.json` describing the project, the same way rust-analyzer
needs a loadable Cargo manifest.

`clangd` also has a version floor the table row above doesn't fully convey:
validation is against upstream 23.1.0. Ubuntu's packaged clangd 18.1.3 does
not implement `callHierarchy/outgoingCalls` — it answers "method not found"
rather than erroring, so callees go silently missing with no warning that
anything is wrong. If clangd is returning callers but never callees, this is
why; check its version.

Servers listed as untested are untested. They are not claimed to be supported.

The integration tests (`cargo test -p lspgraph-core --test integration_servers`)
drive each of the five servers above end to end, and skip the ones that are
not installed on the machine running them. Pull requests run unit tests on
Linux, macOS and Windows, plus lint and an MSRV check; the five-server
integration suite runs nightly rather than on every pull request, because two
intermittent failures were observed during development and never reproduced.

Those two failures were in different suites, and only one of them is now
explained. The unit-suite failure had a real cause: a race in the transport
test harness, since replaced with a deterministic handshake. That harness is
`#[cfg(test)]` code inside the library, so it is not linked into the
integration test binary at all and cannot account for the integration-suite
failure — which remains unexplained. The nightly run keeps its complete log as
an artifact, so that if it happens again there is something to read.

## Configuration

`servers.toml` maps a language to the server that handles it:

```toml
[rust]
cmd = "rust-analyzer"
extensions = ["rs"]
ready_timeout_secs = 300   # optional
```

| Key | Meaning |
|-----|---------|
| `cmd` | Full command line, split on whitespace. |
| `extensions` | File extensions, without the leading dot. |
| `ready_timeout_secs` | How long to wait for the server to become genuinely ready. Default 300. |
| `concurrency` | **Reserved; nothing reads it yet.** Default 1. |

`concurrency` is accepted so that configs written against it keep parsing, but
no code consumes it: every crawl is serial today regardless of what you set.
Bounded-concurrency pipelining is deliberately deferred until it has been
measured — setting this will not make anything faster.

`lspgraph` looks for that file in three places, most specific first:

1. `$LSPGRAPH_SERVERS_TOML`, if set
2. `servers.toml` in the working directory
3. `~/.config/lspgraph/servers.toml` (`%APPDATA%\lspgraph\servers.toml` on
   Windows, `$XDG_CONFIG_HOME` honoured where set)

The third is the one to use for an installed binary — put your servers there
once and `lspgraph` works from any directory. If none exists, the error names
every path it looked in.

## Install

Download a binary from the [releases page][releases], or build from source with
`cargo build --release`.

```sh
# Linux x86_64, statically linked -- runs on any distribution
mkdir -p lspgraph
curl -sL https://github.com/kpanuragh/lspgraph/releases/latest/download/lspgraph-x86_64-unknown-linux-musl.tar.gz \
  | tar xz --strip-components=1 -C lspgraph
./lspgraph/lspgraph rust /path/to/project
```

That URL always points at the newest release, so it does not go stale. Swap the
target for the one you want:

| Platform | Asset |
|----------|-------|
| Linux x86_64 (static) | `lspgraph-x86_64-unknown-linux-musl.tar.gz` |
| Linux aarch64 (static) | `lspgraph-aarch64-unknown-linux-musl.tar.gz` |
| macOS Intel | `lspgraph-x86_64-apple-darwin.tar.gz` |
| macOS Apple Silicon | `lspgraph-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `lspgraph-x86_64-pc-windows-msvc.zip` |

Each archive holds the binary, this README and both licences. Every release also
publishes the same archives with the version in the name, for pinning, and a
`SHA256SUMS` covering both:

```sh
curl -sLO https://github.com/kpanuragh/lspgraph/releases/latest/download/SHA256SUMS
sha256sum -c --ignore-missing SHA256SUMS
```

### Homebrew

```sh
brew tap kpanuragh/tap
brew install lspgraph
```

### From crates.io

```sh
cargo install lspgraph-tui
```

That builds from source and installs a binary called `lspgraph`. The engine is
published separately as [`lspgraph-core`][core] for anyone who wants the call
graph without a terminal interface.

[core]: https://crates.io/crates/lspgraph-core

### Debian and RPM packages

Every release carries `.deb` and `.rpm` packages for x86_64 and aarch64. They
wrap the same statically linked binary as the archives, so they declare no
shared-library dependencies and install regardless of the distribution's glibc
version.

```sh
sudo dpkg -i lspgraph_<version>-1_amd64.deb
sudo rpm -i lspgraph-<version>-1.x86_64.rpm
```

However you install it, `lspgraph` drives a language server and does not bundle
one — see [Configuration](#configuration).

[releases]: https://github.com/kpanuragh/lspgraph/releases/latest

Windows binaries are built and unit-tested in CI, including every terminal
rendering test. The interactive path — raw mode and the alternate screen on a
real Windows console — is not verified by anything, and eight unit tests are
Unix-only and do not run there (six process-lifecycle guards, and two others
that shell out to a Unix command or build a Unix path). Treat Windows as untested in
practice until someone confirms it.

## Try it

```sh
cargo run -p lspgraph-tui -- rust /path/to/a/cargo/project
```

`←/→` move between panes, `↑/↓` select, `Enter` re-centres on the selected
symbol, `u` goes back, `/` searches, `q` quits.

Symbols the language server cannot resolve are shown with a `⊘` marker and an
explanation, rather than hidden — between 2.5% (Rust) and 30% (TypeScript) of
named callables legitimately have no call hierarchy.

If something goes wrong, an overlay shows the message: `esc` dismisses it,
`r` ends the session so you can relaunch (it does not restart the server in
place), and `q` quits.

The engine alone, with no terminal interface, is also reachable through the
`crawl` example:

```sh
cargo run --example crawl -- rust /path/to/a/cargo/project
```

## License

MIT OR Apache-2.0
