# CI and release binaries — design

Date: 2026-09-17
Status: approved, pre-implementation
Depends on: `docs/superpowers/specs/2026-09-16-lspgraph-design.md` (engine),
`docs/superpowers/specs/2026-09-16-lspgraph-tui-design.md` (interface)

## 1. Context

`lspgraph` is built, merged and public: an engine plus a terminal interface,
validated against five language servers. It has no automated testing and no
release artefacts. A user who wants to run it must clone the repository and
build it.

This document specifies two things: continuous integration, and prebuilt
release binaries.

## 2. Scope

Distribution was deliberately split into three pieces. This spec covers the
first two:

- **A. CI** — automated testing on pull requests, and a scheduled run of the
  suite that needs real language servers.
- **B. Release binaries** — tagged releases with prebuilt binaries for five
  targets.
- **C. Package channels** — crates.io, a Homebrew tap, `.deb`/`.rpm`. Deferred
  to its own cycle, because it is blocked on credentials only the repository
  owner can supply: a crates.io API token, and a tap repository with a token
  that can push to it. C also depends on B, since a Homebrew formula points at
  B's release tarballs.

## 3. Goals and non-goals

### Goals

- Every pull request gets unit tests, lint and formatting checked on Linux,
  macOS and Windows.
- The declared MSRV is enforced rather than assumed.
- The five-server integration suite runs on a schedule, and its failures are
  diagnosable.
- A tagged release produces working binaries for five targets, with checksums.

### Non-goals for this cycle

- Publishing to crates.io, Homebrew or any distro repository (that is C).
- Code coverage reporting, benchmarking in CI, or release signing beyond the
  commit signatures already in use.
- Verifying the interactive terminal path on macOS or Windows. See §7.

## 4. Continuous integration

### 4.1 `ci.yml` — on pull request and push to `master`

Three jobs.

**test** — a matrix across `ubuntu-latest`, `macos-latest` and
`windows-latest`, running:

```
cargo test --workspace --lib --bins
```

Unit tests only. `tests/integration_servers.rs` is excluded because it requires
five language servers; it runs in §4.2 instead. This still covers a great deal:
every `ratatui::TestBackend` rendering test is a unit test, so layout, the
unresolved marker and the pending-versus-empty distinction are all verified on
all three platforms.

**lint** — `ubuntu-latest` only:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Clippy is currently clean workspace-wide, so `-D warnings` starts from a clean
slate rather than needing a grandfathering list.

**msrv** — `ubuntu-latest`, Rust 1.88:

```
cargo check --workspace
```

The workspace declares `rust-version = "1.88"`, and this job is what keeps that
number true.

It was written before the number was checked, and checking it found the claim
false. The manifests previously declared 1.75 for the engine and 1.88 only for
the interface, on the reasoning that the engine should stay usable by consumers
on older toolchains. That reasoning did not survive contact with the dependency
tree: `lspgraph-core` depends on `lsp-types`, which depends on `url`, which
pulls in `idna` and the `icu_*` crates — and those require rustc 1.88. The
engine never built on 1.75 once that chain landed, so the split was cosmetic and
the declared floor was simply wrong.

The workspace now declares one MSRV, 1.88, which is the truth. That is precisely
the class of silent drift this job exists to catch; it caught it before it
existed.

### 4.2 `integration.yml` — scheduled and manual

Triggers: a daily `schedule`, and `workflow_dispatch` so it can be run on
demand. `ubuntu-latest` only — the language servers are the point, not the
host platform.

It installs all five servers, writes a `servers.toml` naming them (all will be
on `PATH`, so bare names suffice), and runs:

```
cargo test -p lspgraph-core --test integration_servers -- --test-threads=1 --nocapture
```

`--test-threads=1` because five language servers starting concurrently contend
for CPU and make timings unreliable. `--nocapture` because of §4.3.

Note `gopls` shells out to the Go toolchain, so `go` must be on `PATH`; GitHub's
Ubuntu runners preinstall it. `clangd` needs a compile database, which the C
fixture's test generates at run time.

### 4.3 Why the integration suite is scheduled, not gating

Two intermittent test failures were observed during development — one in the
integration suite, one in the core unit suite — and **neither was ever
reproduced**, across 146 or more subsequent runs. Both times the failure was
recorded but the failing test was not, because only summary lines were
captured.

Two consequences, and they are requirements rather than preferences:

- **The integration suite does not gate pull requests.** A suite that fails
  unpredictably would block merges for reasons unrelated to the change under
  review, and would train everyone to re-run until green — which is how a real
  regression gets waved through.
- **The scheduled workflow must make a failure diagnosable.** It runs with
  `--nocapture` and, on failure, uploads the complete log as a build artifact.
  This is the specific mechanism that prevents a third occurrence of "something
  failed and we do not know what".

Until a failure has actually been captured and understood, this suite is not
described anywhere as stable.

## 5. Release binaries

### 5.1 `release.yml` — on tags matching `v*`

**A version guard runs first.** The tag must match the version in the crate
manifests; if it does not, the workflow fails before building anything.
Publishing a binary labelled with a version it was not built from is worse than
publishing nothing.

**Targets** (five):

| Target | Runner | Notes |
|--------|--------|-------|
| `x86_64-unknown-linux-musl` | ubuntu | static, verified locally |
| `aarch64-unknown-linux-musl` | ubuntu | cross-compiled |
| `x86_64-apple-darwin` | macos | |
| `aarch64-apple-darwin` | macos | Apple Silicon |
| `x86_64-pc-windows-msvc` | windows | see §7 |

A static musl build was verified locally before this spec was written: it needs
no `musl-gcc`, because every dependency is pure Rust. The resulting binary is
2.4MB and reports `static-pie linked`, so it runs on any Linux distribution
regardless of glibc version.

**Packaging.** Each target produces an archive — `.tar.gz` on unix,
`.zip` on Windows — containing the binary, `README.md`, and both licence files.
A `SHA256SUMS` file covers every archive.

**Publishing.** A GitHub Release is created for the tag with all archives and
the checksum file attached.

## 6. Supporting changes

**Licence files are missing.** Both manifests declare
`license = "MIT OR Apache-2.0"`, but no `LICENSE-MIT` or `LICENSE-APACHE` exists
in the repository. The claim is currently unbacked. Both files are added, and
both go into every release archive.

**README** gains install-from-release instructions, and the Windows note
described in §7.

## 7. Windows, stated honestly

CI builds and unit-tests on Windows, which is real verification: the code
compiles, and every rendering test passes against `TestBackend`.

Two things remain unverified there:

- **The interactive terminal path.** crossterm's raw mode and alternate-screen
  handling on a real Windows console is exercised by nothing. The `TestBackend`
  tests render into a buffer and never touch a terminal.
- **Eight `#[cfg(unix)]` tests do not run**, including the process-leak guards
  that prove a language server does not survive the interface exiting.

  This spec said "four" when it was written, by counting `#[cfg(unix)]`
  attributes rather than the tests behind them — one of the four gates a module
  of six. CI settles it: the unit suite reports 78 passed on Ubuntu and 70 on
  Windows. The implementation added one further gate, so the real figure is
  eight, and the README states that number.

So the README states that Windows binaries are built and unit-tested but that
the interactive path is unverified, until someone confirms it on real hardware.

This matters because the README's support table has been honest about exactly
this distinction from the beginning — it already separates "validated against
the real server" from "expected to work, untested". Shipping a Windows binary
while implying the same confidence as Linux would be the first claim in this
project that was not earned.

## 8. Open risks

- **macOS, Windows and aarch64 builds get their first real test in CI.** None
  can be verified on the development machine. The workflows are written to be
  correct, but a first run that fails on one of those targets is an expected
  outcome, not a surprise — it will be fixed against real CI output rather than
  guessed at.
- **The `aarch64-unknown-linux-musl` cross-compile** may need a linker that the
  Ubuntu runner does not have by default. If so, the fix is a cross-compilation
  action or `cross`; the target is not dropped silently.
- **The scheduled integration run will eventually catch the flake.** That is its
  purpose. The first time it fires, the artifact is the evidence needed to
  diagnose it, and the outcome should be a fix rather than a re-run.
