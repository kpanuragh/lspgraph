# lspgraph-tui

The terminal interface for [lspgraph](https://github.com/kpanuragh/lspgraph).
It installs a binary called `lspgraph`.

Point it at a repository and a language, search for a symbol, and walk callers
and callees:

```sh
cargo install lspgraph-tui
lspgraph rust /path/to/project
```

Three panes: callers on the left, the focused symbol in the middle, callees on
the right. Enter re-centres on a neighbour, `u` walks back, `/` searches.

It reads no source code of its own — the graph comes from whichever language
server you configure in `servers.toml`, via
[lspgraph-core](https://crates.io/crates/lspgraph-core).

Prebuilt binaries for Linux, macOS and Windows are on the
[releases page](https://github.com/kpanuragh/lspgraph/releases/latest). Windows
is built and unit-tested, but its interactive terminal path is unverified.

Licensed under MIT or Apache-2.0, at your option.
