# lspgraph-tui — design

Date: 2026-09-16
Status: approved, pre-implementation
Depends on: `docs/superpowers/specs/2026-09-16-lspgraph-design.md` (the engine spec)

## 1. Context

`lspgraph-core` is built and merged: it drives any LSP server's call-hierarchy
API and exposes a lazily-expanded, memoized call graph. It has no user
interface. This document specifies the terminal interface that makes it
usable, which the engine spec names as the v0.1 binary.

Two measurements from the engine's feasibility work shape everything below:

- Warm expansion is 6–31ms (p50), so navigation can feel instant.
- Readiness against a live server takes 3–15s in the common case, and roughly
  28s on a first-ever open of a Rust project while cargo fetches dependencies.
  A cold expansion can take seconds.

The first number is why the interface can be interactive at all. The second is
why it cannot call the engine from its render loop.

## 2. The entry-point gap, and how it is closed

The engine can only produce a `NamedCallable` by enumerating one file's
document symbols. There is no way to ask "where is `handle_request`?". Without
solving that, a user must already know which file to open, which defeats the
purpose of a tool for reading unfamiliar code.

So `lspgraph-core` gains one capability:

```rust
pub struct SymbolMatch {
    pub name: String,
    pub container: Option<String>,
    pub uri: Url,
    pub position: Position,
    pub kind: SymbolKind,
}

impl LanguageServer {
    pub fn workspace_symbols(&self, query: &str) -> Result<Vec<SymbolMatch>>;
}
```

This belongs in core, not the TUI: it is an LSP capability, and the crate split
exists to keep LSP knowledge out of the interface.

Results pass through the existing `symbols::is_named_callable` filter, so
search inherits the anonymous-callback rejection already proven against
tsserver (where that noise was 74% of enumerated entries).

**Degradation is a requirement, not a nicety.** `workspace/symbol` is optional
in LSP and some servers do not implement it. If `initialize` does not advertise
`workspaceSymbolProvider`, the interface says so in the search pane and falls
back to enumerating the currently-open file. It must never render an empty
result list in that case, because "this server cannot search" and "your query
matched nothing" would then look identical — the same class of
indistinguishable-failure problem the engine spec's §5.2 exists to prevent.

There is a second, narrower version of the same problem, measured against a
live server rather than inferred: vtsls only returns `workspace/symbol`
matches for files that have already been opened with `textDocument/didOpen`;
rust-analyzer and basedpyright do not require this. `workspace_symbols`
deliberately does not open files itself to compensate, since that would make
every search pay for a full enumeration — precisely the cost `workspace/symbol`
exists to avoid — so on a server with this dependency an empty result means
"nothing matched among the files opened so far," not "nothing matched in the
repository." A search view built on top of this must not present an empty
result as authoritative on such a server; it needs its own signal (e.g. noting
how much of the repository is actually covered) rather than rendering the
empty list as if it were conclusive.

## 3. Threading

The engine runs on a worker thread. The UI thread owns no `Engine` and never
blocks on one.

```
UI thread (ratatui)                 worker thread (owns Engine)
───────────────────                 ───────────────────────────
Request ──────────────────────────► Search(String)
                                    Seed(SymbolMatch)
                                    Expand(NodeId)
                                    Restart
                                    Shutdown
◄────────────────────────── Event   Progress(String)
                                    Ready
                                    Matches(Vec<SymbolMatch>)
                                    Seeded(Option<NodeId>)
                                    Expanded(NodeId, Expansion)
                                    Failed(NodeId, String)
                                    Fatal(String)
```

Two `std::sync::mpsc` channels. No async runtime, consistent with the engine's
own constraint. The UI thread renders whatever state it currently holds and
shows progress for anything outstanding.

The worker calls `Engine::shutdown` when it exits, so the language server dies
with the interface. That method exists because the engine's final review found
a leak: a server moved into an `Engine` was previously unreachable and
`Child::drop` does not kill a process. A long-lived interface is exactly the
consumer that would have turned that leak into accumulating multi-gigabyte
processes.

## 4. Screens

A three-state machine:

- **Starting** — server spawning and readiness polling. Shows the progress
  messages the worker forwards. This state can legitimately last 15 seconds,
  and around 28s on a first-ever open of a Rust project, so it must show what
  it is waiting for rather than an undifferentiated spinner — a silent 28-second
  wait is indistinguishable from a hang.
- **Search** — a query box and a live result list.
- **Graph** — the three-pane view below.

Errors render as an overlay over whatever is behind them. No error is ever
rendered as an empty state.

## 5. The graph view

```
 lspgraph · rust · ripgrep          root › middle › send_request
┌ callers ───────┬ send_request ──────┬ callees ──────────┐
│ handle_request │ fn send_request(   │ validate_token    │
│ retry_worker   │   req: Request,    │ build_headers     │
│ flush_queue    │ ) -> Result<Resp>  │ ⊘ over(x: number) │
│                │ transport.rs:88    │   http.post       │
└────────────────┴────────────────────┴───────────────────┘
 ←/→ pane · ↑/↓ select · ⏎ re-centre · u back · / search · q quit
```

The centre pane shows the focused symbol's `detail` (its signature, as the
language server reports it) and its file and line. The side panes list its
callers and callees.

Selecting a neighbour and pressing Enter re-centres the view on it: the
selected node becomes the centre, and its own callers and callees are fetched.
The breadcrumb across the top records the path taken, and `u` walks back along
it.

### Keybindings (v0.1, not configurable)

| Key | Action |
|-----|--------|
| `←` / `→` | move between panes |
| `↑` / `↓` | move selection within a pane |
| `Enter` | re-centre on the selected node |
| `u` | back, one step along the breadcrumb |
| `/` | open search |
| `r` | restart the language server (only offered after `ServerExited`) |
| `q` | quit |

### Pending is not empty

A pane whose expansion is still in flight renders a spinner, never an empty
list. An empty list means "this symbol genuinely has no callers", which is a
real and useful answer; conflating it with "still loading" would make the
interface lie during exactly the seconds a cold expansion takes.

### Unresolved nodes are marked, never hidden

Engine spec §5.5 requires that symbols which do not resolve are shown rather
than dropped — between 2.5% (Rust) and 30% (TypeScript) of named callables
legitimately do not resolve. In this interface that means a `⊘` marker beside
the name, with the reason in the status line when selected:

- `NoCallHierarchyItem` → "no call hierarchy: likely an overload signature or
  an anonymous function"
- `TransientContentModified` → "the server repeatedly reported content
  modified; this symbol may resolve on a later attempt"

Hiding them would reproduce, at the interface layer, the exact dishonesty the
engine spec spent effort eliminating.

## 6. Error handling

| Engine error | What the user sees |
|--------------|--------------------|
| `NoCallHierarchy` | Named refusal at startup, then exit: this server does not support call hierarchy |
| `NotReady` | "Server not ready after Ns; tried K candidate symbols" — the diagnostic the engine already produces |
| `NoCandidates` | Distinguishes "the server answered nothing for any file" from "this codebase has no callable symbols", using the `files_attempted` / `files_failed` counts |
| `ServerExited` | A banner plus `r` to restart. The graph already built stays on screen — it is still true, just no longer growable |
| `ContentModified` | Invisible. Core already retries it, and only a persistent failure surfaces, as an unresolved node |

## 7. Testing

- `ratatui::TestBackend` renders into an inspectable buffer, so layout, the
  breadcrumb, the `⊘` marker and the spinner-versus-empty distinction are all
  assertable without a terminal.
- The worker is a pure request/event state machine over a trait-object engine
  handle, so its protocol is testable with a stub and no language server.
- Live behaviour is already covered by the engine's integration suite; this
  crate does not duplicate it.

The pending-versus-empty distinction and the unresolved marker get explicit
tests. Both are places where a plausible-looking regression would silently
mislead the user rather than break anything.

## 8. Dependencies

`ratatui` and `crossterm`, plus `lspgraph-core`. Nothing else. The engine's
no-async-runtime constraint carries over unchanged.

## 9. Non-goals for v0.1

Explicitly out of scope: editor integration of any kind, mouse support,
configurable keybindings, graph export, drawn graph layout, multiple
repositories at once, and wiring the cache (which remains unwired — see the
engine spec §5.4 and `cache.rs`'s module documentation for what a consumer must
do first).

## 10. Open risks

- **`workspace/symbol` quality varies more than call hierarchy does.** It is
  validated here against rust-analyzer, vtsls and basedpyright only. Servers
  that implement it poorly will produce a weak search experience, and the
  fallback path is the only defence.
- **Interface latency under a cold cache is unmeasured.** Expansion is 6–31ms
  warm, but the first expansion of a session pays the cold cost, and no
  measurement exists of how that feels in a UI rather than in a benchmark.
- **The engine's integration suite has one failure observed once and never
  reproduced** (a fast failure, not a readiness timeout, whose error was not
  captured). It is unrelated to this interface but shares the suite, and
  should be made diagnosable before either is trusted in CI.
