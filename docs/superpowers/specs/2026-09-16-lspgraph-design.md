# lspgraph — design

Date: 2026-09-16
Status: approved, pre-implementation
Working title: `lspgraph` (name not final; nothing depends on it)

## 1. Problem

Reading an unfamiliar codebase means answering "what calls this, and what does
this call?" repeatedly until a mental model forms. Sourcetrail did exactly that
and was archived by its authors at the end of 2021. Users have not replaced it:
they still run 2019 binaries, and the recurring complaints are that it was
abandoned and that it never supported Go.

Sourcetrail died of its own architecture. It shipped a hand-written indexer per
language — clang-based for C/C++, separate indexers for Java and Python. Each
was a compiler-engineering project in its own right, which capped the tool at
four languages and made maintenance unsustainable. The only active successor,
NumbatUI, is a fork of the original C++ codebase and inherits that same cost.

## 2. Approach

Do not write indexers. Be a Language Server Protocol client.

LSP 3.16 added call hierarchy:

- `textDocument/prepareCallHierarchy` — resolve the symbol at a position
- `callHierarchy/incomingCalls` — who calls it
- `callHierarchy/outgoingCalls` — what it calls

That is the graph, already computed, exposed as structured data. rust-analyzer,
gopls, clangd, vtsls, basedpyright and others each maintain the hard language
work inside their own communities. The maintenance burden that killed
Sourcetrail becomes, by construction, somebody else's.

## 3. Feasibility evidence

A throwaway probe (raw JSON-RPC, no dependencies) measured three servers
against three real repositories on 2026-09-16.

| Lang | Server | Repo | Named fns | Resolve | warm p50 | warm p90 | warm rate | Full crawl |
|------|--------|------|-----------|---------|----------|----------|-----------|------------|
| Rust | rust-analyzer | ripgrep (56k LOC) | 3028 | 97.5% | 6ms | 25ms | 25.4/s | 2.0 min |
| TS | vtsls | zod/src | 2028 | 70% | 31ms | 55ms | 23.3/s | 1.5 min |
| Python | basedpyright | requests (6.4k LOC) | 248 | 100% | 11ms | 38ms | 32.7/s | 0.1 min |

Cold start to first semantic answer: vtsls 4.0s, basedpyright 3.4s,
rust-analyzer 1–12s with a warm cargo cache and roughly 28s on a first-ever
open (dominated by dependency fetch). Zero protocol errors across all runs.

Three findings from that probe drive the design below, and each is recorded
where it applies:

1. Warm p50 of 6–31ms means interactive expansion is imperceptible; only
   exhaustive precomputation is slow. See §5.1.
2. There is no portable readiness signal, and the obvious heuristics fail in
   ways that look identical to a dead server. See §5.2.
3. `documentSymbol` enumerates anonymous callbacks that call hierarchy
   correctly refuses. See §5.3.

## 4. Goals and non-goals

### Goals

- Navigate a call graph interactively, expanding from the symbol under the
  cursor, with no blocking full-repository index.
- Work with any language server that implements call hierarchy, configured in
  a file, with no per-language code in this project.
- Ship with rust-analyzer, vtsls and basedpyright validated in CI.
- Be honest in the UI about what did not resolve and why.

### Non-goals for v0.1

Explicitly out of scope, to be revisited only after v0.1 ships:

- Type hierarchy (`prepareTypeHierarchy`, supertypes/subtypes)
- Graph export in any format
- Drawn graph layout; v0.1 is three panes, not a canvas
- Editor plugins of any kind
- The MCP server
- Multi-root workspaces
- Any write operation (rename, refactor, edit)

## 5. Architecture

```
lspgraph-core/     engine; no UI and no terminal dependencies
lspgraph-tui/      ratatui consumer; the v0.1 binary
lspgraph-mcp/      deferred, not part of v0.1
```

The crate split is load-bearing rather than decorative. The engine must be
exercised against real language servers in CI; the TUI must be looked at by a
human. Separating them is what makes the engine testable at all.

### Core modules

| Module | Responsibility |
|--------|----------------|
| `transport` | JSON-RPC over stdio, `Content-Length` framing, request/response correlation, notification stream, and replying to server→client requests |
| `server` | Process lifecycle, `initialize` handshake, capability negotiation, shutdown |
| `readiness` | Multi-candidate semantic polling until the server genuinely answers |
| `symbols` | Enumeration via `documentSymbol`, filtered to named callables |
| `graph` | Node and edge model, stable symbol identity |
| `engine` | Lazy expansion with memoization; the public API |
| `cache` | Cross-session persistence with per-file invalidation |

`transport` must answer server→client requests rather than ignore them. The
probe observed `workspace/configuration` and `client/registerCapability`;
replying with a null result was sufficient to keep every tested server healthy.

### 5.1 Lazy expansion is the core abstraction

The public API is:

```rust
fn expand(&mut self, node: NodeId) -> Result<Expansion>;  // callers + callees
```

memoized per node. There is deliberately no "index this repository" call on the
hot path. Background warming exists but is opt-in, cancellable, and never
blocks a user action.

This is the design Sourcetrail structurally could not have had. It owned its
index, so it paid full construction cost up front. An LSP client does not own
the index and therefore does not have to. Measured full-repository crawls of
1.5–2.0 minutes are acceptable as an optional background task and would be
unacceptable as a startup cost — so they are never a startup cost.

### 5.2 Readiness is a first-class module

No standard readiness signal exists. rust-analyzer's `rustAnalyzer/cachePriming`
progress token is proprietary to it; other servers differ or are silent.

Two failure modes were observed directly, and both must be designed against:

- **Quiet-period heuristics are wrong.** rust-analyzer emitted a single
  `Fetching begin` notification at t=0.0s and then went *completely silent for
  17 seconds* while fetching dependencies. A "6 seconds of quiet means ready"
  rule fired at t=7.2s, after which every semantic request returned empty —
  indistinguishable from a server that does not support call hierarchy.
- **Single-target polling is wrong.** Some symbols legitimately never resolve.
  Polling one such target cannot be distinguished from a dead server, and in
  the probe this produced a 600-second false failure against a perfectly
  healthy vtsls.

The portable answer, and the required behaviour: gather candidate targets
spread across the repository — up to 3 symbols from each of up to 40 files,
sampled at an even stride rather than taken alphabetically — poll them in
rotation with a real `prepareCallHierarchy` request, and treat the server as
ready when **any** candidate answers. Both bounds and the overall readiness
timeout (default 300s) are configurable. On timeout, report which candidates
were tried, so the failure is diagnosable rather than a hang.

Even striding matters: the probe's alphabetical selection landed entirely
inside a benchmark directory whose imports could not resolve, which is how the
600-second false failure happened.

### 5.3 Enumeration must filter to named callables

`documentSymbol` is not a list of callable declarations. tsserver synthesizes
entries for anonymous callbacks with names such as `expect() callback`,
`test("...") callback` and `on("cycle") callback`. `prepareCallHierarchy`
correctly refuses them, because they are expressions rather than named
declarations.

In zod's source this noise was 74% of all enumerated "functions". Filtering to
entries whose name is a plain identifier took the resolve rate from 14% to 70%
and warm throughput from 7.5 to 23.3 symbols/second.

v0.1 filter: symbol kind is Function (12) or Method (6), and the name matches
`^[A-Za-z_$][A-Za-z0-9_$]*$`.

### 5.4 Symbol identity

`CallHierarchyItem` carries `{uri, name, kind, range, selectionRange, detail}`.
Node identity in v0.1 is the triple `(uri, selectionRange.start, name)`.

Ranges shift when a file is edited. v0.1 therefore invalidates every node
belonging to an edited file rather than attempting to fix up stored ranges.
Correct and coarse is preferred over clever and subtly wrong; finer-grained
invalidation is a v0.2 concern and only if profiling justifies it.

The cache persists per repository under the platform cache directory
(`$XDG_CACHE_HOME/lspgraph/<hash-of-repo-path>/` on Linux), keyed by file path
and content hash. A cache whose schema version does not match the running
binary is discarded rather than migrated. Nothing in the cache is
authoritative: a cold cache only costs time, never correctness.

### 5.5 Non-resolving symbols are shown, never dropped

Between 2.5% (Rust) and 30% (TypeScript) of *named* callables legitimately
return no call hierarchy item. Observed causes: overload signatures such as
`gt(value: number): this;`, and arrow-function assignments such as
`inst.max = (value, params) => ...`.

These render as explicit leaf nodes carrying a reason. They are never silently
omitted. Silently dropping a third of a TypeScript codebase would make the tool
quietly misleading, which is worse than being visibly incomplete.

### 5.6 Concurrency is measured, not assumed

The probe crawled strictly serially. Bounded-concurrency pipelining is a
plausible speedup for background warming, and language servers do accept
concurrent requests, but this has not been measured.

v0.1 ships a configurable concurrency limit whose default is serial, plus a
benchmark to establish the real number. The default changes when there is
evidence, not before.

## 6. Configuration

A `servers.toml` maps language identifiers to server commands. Adding a
language is adding an entry; it requires no code in this project.

```toml
[rust]
cmd = "rust-analyzer"
extensions = ["rs"]

[typescript]
cmd = "vtsls --stdio"
extensions = ["ts", "tsx"]

[python]
cmd = "basedpyright-langserver --stdio"
extensions = ["py"]
```

The README states plainly which servers are validated in CI and which are
merely expected to work. Untested servers are never described as supported.

## 7. Error handling

| Condition | Behaviour |
|-----------|-----------|
| Server does not advertise `callHierarchyProvider` | Detected at `initialize`; refuse immediately with a message naming the server |
| Readiness never achieved | Hard timeout, then a diagnostic listing every candidate tried |
| Server exits mid-session | Surface it, offer restart, retain the cache |
| Request timeout | Per-request budget; degrades one node, never the session |
| Symbol does not resolve | Rendered as a leaf with a reason (§5.5) |

## 8. Testing

`transport` and `graph` are pure logic and get ordinary unit tests.

`readiness` and `symbols` are meaningful only against a real server, so they get
integration tests gated on server availability, with a CI matrix installing
rust-analyzer, vtsls and basedpyright. Small fixture repositories are committed
to the tree so results are reproducible.

Fixtures must include the awkward cases the probe surfaced: anonymous
callbacks, overload signatures, and arrow-function assignments. These are the
behaviours most likely to regress silently.

The probe harness becomes the seed of the benchmark suite, so the numbers in §3
stay honest over time.

## 9. Open risks

- **Scale is unverified.** The largest repository tested was 56k LOC. Behaviour
  at kernel or large-monorepo scale is unknown, and this is the risk most
  likely to invalidate assumptions in §5.1.
- **Three servers proven, others assumed.** clangd, gopls and jdtls are
  untested here; no toolchains were available. Go and C/C++ are precisely the
  audiences the Sourcetrail discussion asked for, so this gap matters.
- **Edit invalidation is untested.** No measurement exists of re-expansion cost
  after a file changes.

## 10. After v0.1

In rough priority order, each contingent on v0.1 proving useful: the MCP server
over the same core; type hierarchy; validation of gopls and clangd; finer-grained
cache invalidation.
