# lspgraph

Explore a codebase's call graph by driving any LSP server's call hierarchy API.

Sourcetrail did this well and was archived in 2021. It maintained a
hand-written indexer per language, which capped it at four languages and made
maintenance unsustainable. lspgraph writes no indexers: it is an LSP client, so
every language with a language server is reachable.

## Status

Early. `lspgraph-core` is the engine; the TUI is not written yet.

## Server support

Adding a language means adding a table to `servers.toml`. No code.

| Server | Status |
|--------|--------|
| rust-analyzer | validated in CI |
| vtsls | validated in CI |
| basedpyright | validated in CI |
| anything else implementing `callHierarchyProvider` | expected to work, untested |

Servers listed as untested are untested. They are not claimed to be supported.

## Try it

```sh
cargo run --example crawl -- rust /path/to/a/cargo/project
```

## License

MIT OR Apache-2.0
