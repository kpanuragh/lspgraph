# lspgraph-core

The engine behind [lspgraph](https://github.com/kpanuragh/lspgraph): it drives
any Language Server Protocol server's call-hierarchy API and exposes the result
as a lazily expanded, memoized call graph.

It contains no per-language code. Every language comes from a language server
that already does the work, so support is a configuration entry rather than an
indexer.

```rust
let server = LanguageServer::start("rust", &cfg, root)?;
let mut engine = Engine::new(server);
let expansion = engine.expand(node)?;   // callers and callees, memoized
```

Validated end to end against rust-analyzer, vtsls, basedpyright, gopls and
clangd.

Two behaviours worth knowing before you depend on it:

- **Readiness is polled semantically**, not guessed from a quiet period. A
  server that has gone silent for six seconds may simply be fetching
  dependencies.
- **Symbols that do not resolve are reported, never dropped.** Between 2.5% and
  30% of named callables legitimately have no call hierarchy, so they arrive as
  leaves carrying a reason.

See the [repository](https://github.com/kpanuragh/lspgraph) for the terminal
interface, configuration and the full design notes.

Licensed under MIT or Apache-2.0, at your option.
