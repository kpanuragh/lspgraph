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
| anything else implementing `callHierarchyProvider` | expected to work, untested |

Servers listed as untested are untested. They are not claimed to be supported.

The integration tests (`cargo test -p lspgraph-core --test integration_servers`)
drive each of the three servers end to end, and skip the ones that are not
installed on the machine running them. There is no CI pipeline yet, so the
validation above is whatever was last run by hand.

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

To point at servers installed outside `PATH` without editing the committed
file, set `LSPGRAPH_SERVERS_TOML` to an override file.

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
