# CI and Release Binaries Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `lspgraph` automated testing on every pull request and prebuilt binaries on every tag.

**Architecture:** Three GitHub Actions workflows. `ci.yml` runs unit tests, lint and an MSRV check on pull requests. `integration.yml` runs the five-language-server suite on a schedule, uploading its full log on failure. `release.yml` builds five targets on a tag and publishes archives with checksums.

**Tech Stack:** GitHub Actions, `dtolnay/rust-toolchain`, `Swatinem/rust-cache`, `actions/upload-artifact`, `softprops/action-gh-release`. Local validation with `actionlint`.

**Spec:** `docs/superpowers/specs/2026-09-17-lspgraph-ci-release-design.md`

## Global Constraints

- **Workspace MSRV is 1.88.** The previous per-crate split (core 1.75, tui 1.88) was measured and found false — `lspgraph-core` reaches 1.88 anyway through `lsp-types` → `url` → `idna` → `icu_*`. One MSRV, declared at the workspace, inherited by both crates.
- No Claude/AI attribution in commit messages, workflow files, or the README. Messages end at the last line of their body.
- Never commit anything under `.superpowers/`. Do not commit `servers.local.toml` (gitignored).
- Workflows pin actions to a major version tag (`@v4`), not `@master`.
- **A workflow cannot be unit-tested.** Every workflow task's verification is: lint locally with `actionlint`, push the branch, then observe the real run with `gh run watch`. A task is not done because the YAML looks right.

## File Structure

```
.github/workflows/
  ci.yml             unit tests (3 OSes), lint, MSRV — on PR and push to master
  integration.yml    five real language servers — scheduled and on demand
  release.yml        five build targets, archives, checksums — on tag
LICENSE-MIT          currently missing; both manifests already claim it
LICENSE-APACHE       currently missing; both manifests already claim it
```

---

### Task 1: Tell the truth in the manifests

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/lspgraph-tui/Cargo.toml`
- Modify: `crates/lspgraph-core/Cargo.toml`
- Create: `LICENSE-MIT`
- Create: `LICENSE-APACHE`

**Interfaces:**
- Consumes: nothing.
- Produces: a workspace `rust-version = "1.88"` that both crates inherit; `LICENSE-MIT` and `LICENSE-APACHE` at the repository root, which Task 4 packages into every release archive.

Two claims in this repository are currently false, and CI would fail on both on its first run. This task makes them true before anything gates on them.

- [ ] **Step 1: Prove the MSRV claim is false**

Run:
```bash
rustup toolchain install 1.75 --profile minimal
cargo +1.75 check -p lspgraph-core
```
Expected: FAILS with `icu_collections@2.3.0 requires rustc 1.88` (and siblings). This is the evidence; record the output in your report.

- [ ] **Step 2: Prove 1.88 is the real floor**

Run:
```bash
rustup toolchain install 1.88 --profile minimal
cargo +1.88 check --workspace
```
Expected: PASSES. If it does not, STOP and report — the floor is higher than this plan assumes and every later task's MSRV number is wrong.

- [ ] **Step 3: Correct the manifests**

In `Cargo.toml`, change the workspace package MSRV:

```toml
[workspace.package]
edition = "2021"
rust-version = "1.88"
license = "MIT OR Apache-2.0"
```

In `crates/lspgraph-tui/Cargo.toml`, replace its standalone MSRV line with the inherited one, so there is a single source of truth:

```toml
rust-version.workspace = true
```

Delete the comment above it that explains the split (it described a distinction that no longer exists). `crates/lspgraph-core/Cargo.toml` already uses `rust-version.workspace = true` and needs no change — confirm that rather than assuming it.

- [ ] **Step 4: Add the licence files**

Both manifests declare `license = "MIT OR Apache-2.0"` and neither licence text exists. Create `LICENSE-MIT` with the standard MIT text, copyright line `Copyright (c) 2026 K Panuragh`. Create `LICENSE-APACHE` with the standard Apache License 2.0 text.

Fetch canonical texts rather than typing them from memory:
```bash
curl -sL https://raw.githubusercontent.com/rust-lang/rust/master/LICENSE-APACHE -o LICENSE-APACHE
curl -sL https://raw.githubusercontent.com/rust-lang/rust/master/LICENSE-MIT -o LICENSE-MIT
```
Then edit `LICENSE-MIT`'s copyright placeholder to read `Copyright (c) 2026 K Panuragh`. Check `LICENSE-APACHE` for any placeholder and leave the Apache text itself unmodified — it is not meant to be edited.

- [ ] **Step 5: Make the tree rustfmt-clean**

`cargo fmt --all --check` currently FAILS (there are real diffs, including in `crates/lspgraph-core/examples/crawl.rs`). Task 2 makes that a gating check, so it must pass first.

Run:
```bash
cargo fmt --all --check    # observe the failures
cargo fmt --all            # apply them
cargo fmt --all --check    # must now be silent
```

- [ ] **Step 6: Verify nothing broke**

Run:
```bash
cargo test --workspace --lib --bins
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: 78 + 65 tests pass, clippy silent. Report the full output, not summary lines.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/lspgraph-core/Cargo.toml crates/lspgraph-tui/Cargo.toml LICENSE-MIT LICENSE-APACHE
git add -u
git commit -m "Declare the MSRV that is actually true, and add the licence texts

The workspace claimed rust-version 1.75 for the engine and 1.88 only for the
interface, so that the engine would stay usable on older toolchains. Measuring
it showed the claim was false: lsp-types pulls in url, idna and the icu crates,
which require 1.88, so the engine never built on 1.75 once that chain landed.
One workspace MSRV of 1.88 replaces the split.

Both manifests also declared MIT OR Apache-2.0 with no licence text present."
```

---

### Task 2: `ci.yml` — tests, lint, MSRV

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: the workspace MSRV of 1.88 and the rustfmt-clean tree from Task 1.
- Produces: a required-status-check surface for pull requests.

- [ ] **Step 1: Install actionlint for local validation**

The Go toolchain is available. Install it so workflow errors surface before a push rather than after:

```bash
go install github.com/rhysd/actionlint/cmd/actionlint@latest
"$HOME/go/bin/actionlint" --version
```
If `go` is not on `PATH`, it is at `~/.local/go/bin/go` on this machine.

- [ ] **Step 2: Write the workflow**

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  pull_request:
  push:
    branches: [master]

env:
  CARGO_TERM_COLOR: always

jobs:
  test:
    name: test (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      # Unit tests only. tests/integration_servers.rs needs five language
      # servers and runs in integration.yml instead.
      - name: Unit tests
        run: cargo test --workspace --lib --bins

  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: Formatting
        run: cargo fmt --all --check
      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings

  msrv:
    name: msrv (1.88)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      # This job is what keeps the declared MSRV true. It was added because the
      # previously declared floor turned out to be false.
      - uses: dtolnay/rust-toolchain@1.88
      - uses: Swatinem/rust-cache@v2
      - name: The workspace must build on its declared MSRV
        run: cargo check --workspace
```

- [ ] **Step 3: Lint it locally**

Run:
```bash
"$HOME/go/bin/actionlint" .github/workflows/ci.yml
```
Expected: no output. Fix anything it reports before pushing — this is the only check available without burning a CI run.

- [ ] **Step 4: Push a branch and watch the real run**

```bash
git checkout -b ci/add-workflows
git add .github/workflows/ci.yml
git commit -m "Add CI: unit tests on three platforms, lint, and an MSRV check"
git push -u origin ci/add-workflows
gh run watch --exit-status
```

Expected: all three jobs pass. `gh run watch --exit-status` returns non-zero if any job fails.

**If a job fails, fix it against the real log rather than guessing:**
```bash
gh run view --log-failed
```
The macOS and Windows runners are the likely first failures — neither has ever built this code. Report what failed and what you changed.

- [ ] **Step 5: Commit any fixes and confirm green**

Re-push and re-watch until `gh run watch --exit-status` succeeds. Record the final run URL in your report.

---

### Task 3: `integration.yml` — the five-server suite

**Files:**
- Create: `.github/workflows/integration.yml`

**Interfaces:**
- Consumes: `LSPGRAPH_SERVERS_TOML`, which `main.rs` and the integration tests already honour.
- Produces: a scheduled signal on the five-language validation, plus a downloadable log artifact when it fails.

This workflow exists in this shape for a specific reason. Two intermittent test failures were seen during development and **neither was ever reproduced**, because only summary lines were captured. It therefore runs off the critical path and keeps the whole log.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/integration.yml`:

```yaml
name: Integration (real language servers)

# Deliberately not on pull_request. Two intermittent failures were observed
# during development and never reproduced; a suite that fails unpredictably
# must not block merges, because that trains everyone to re-run until green.
on:
  schedule:
    - cron: "0 3 * * *"
  workflow_dispatch:

env:
  CARGO_TERM_COLOR: always

jobs:
  integration:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rust-analyzer
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version: "20"
      - uses: actions/setup-python@v5
        with:
          python-version: "3.12"
      - uses: actions/setup-go@v5
        with:
          go-version: stable

      - name: Install language servers
        run: |
          set -euxo pipefail
          npm install -g @vtsls/language-server
          pip install basedpyright
          go install golang.org/x/tools/gopls@latest
          sudo apt-get update
          sudo apt-get install -y clangd
          echo "$HOME/go/bin" >> "$GITHUB_PATH"

      - name: Record server versions
        run: |
          set -x
          rust-analyzer --version
          vtsls --version || true
          basedpyright-langserver --version || true
          "$HOME/go/bin/gopls" version
          clangd --version
          go version

      - name: Write the CI server configuration
        run: |
          cat > servers.ci.toml <<'TOML'
          [rust]
          cmd = "rust-analyzer"
          extensions = ["rs"]

          [typescript]
          cmd = "vtsls --stdio"
          extensions = ["ts", "tsx"]

          [python]
          cmd = "basedpyright-langserver --stdio"
          extensions = ["py"]

          [go]
          cmd = "gopls"
          extensions = ["go"]

          [c]
          cmd = "clangd"
          extensions = ["c", "h"]
          TOML

      # --test-threads=1 because five servers starting at once contend for CPU
      # and make timings unreliable. --nocapture and tee because the whole
      # point of this workflow is that a failure leaves evidence behind.
      - name: Integration suite
        env:
          LSPGRAPH_SERVERS_TOML: servers.ci.toml
        run: |
          set -o pipefail
          cargo test -p lspgraph-core --test integration_servers -- \
            --test-threads=1 --nocapture 2>&1 | tee integration.log

      - name: Upload the full log when it fails
        if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: integration-log
          path: integration.log
          retention-days: 30
```

- [ ] **Step 2: Lint it**

```bash
"$HOME/go/bin/actionlint" .github/workflows/integration.yml
```
Expected: no output.

- [ ] **Step 3: Run it on demand and watch**

`workflow_dispatch` only appears once the workflow is on the default branch OR you dispatch it against your branch with `--ref`. Push first, then:

```bash
git add .github/workflows/integration.yml
git commit -m "Add a scheduled integration run against five real language servers"
git push
gh workflow run integration.yml --ref ci/add-workflows
sleep 10
gh run watch --exit-status
```

Expected: all six integration tests pass — `rust_call_chain`, `typescript_call_chain`, `python_call_chain`, `go_call_chain`, `c_call_chain`, and `typescript_unresolvable_symbols_are_recorded_not_dropped`.

**If any language fails**, the most likely causes are environmental, not code: a server binary missing from `PATH`, `go` absent for gopls, or clangd unable to find a compile database. `gh run view --log-failed` shows which. Fix the workflow, not the tests.

- [ ] **Step 4: Prove the artifact upload works**

The failure path is the whole point of this workflow, so do not leave it untested. Temporarily break the suite, push, run, and confirm the artifact appears:

```bash
# Temporarily add a failing assertion to one test, e.g. in go_call_chain:
#   assert!(false, "deliberate failure to prove the artifact upload works");
git commit -am "TEMPORARY: prove the integration log artifact uploads"
git push
gh workflow run integration.yml --ref ci/add-workflows
gh run watch || true
gh run view --json databaseId -q '.databaseId' | xargs -I{} gh api repos/kpanuragh/lspgraph/actions/runs/{}/artifacts --jq '.artifacts[].name'
```
Expected: an artifact named `integration-log`. Then revert the temporary failure:
```bash
git revert --no-edit HEAD
git push
```
Report whether the artifact appeared. If it did not, the workflow does not do the one thing it was written for.

---

### Task 4: `release.yml` — five targets, archives, checksums

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `LICENSE-MIT` and `LICENSE-APACHE` from Task 1.
- Produces: a GitHub Release per tag, with five archives and `SHA256SUMS`.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    tags: ["v*"]

env:
  CARGO_TERM_COLOR: always

permissions:
  contents: write

jobs:
  # Publishing a binary labelled with a version it was not built from is worse
  # than publishing nothing, so this runs before anything is built.
  guard:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: The tag must match the crate version
        run: |
          set -euo pipefail
          tag="${GITHUB_REF_NAME#v}"
          crate=$(grep -m1 '^version' crates/lspgraph-tui/Cargo.toml | sed -E 's/.*"(.*)".*/\1/')
          echo "tag=$tag crate=$crate"
          if [ "$tag" != "$crate" ]; then
            echo "::error::tag $tag does not match crate version $crate"
            exit 1
          fi

  build:
    needs: guard
    name: ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-unknown-linux-musl
            os: ubuntu-latest
          - target: aarch64-unknown-linux-musl
            os: ubuntu-latest
          - target: x86_64-apple-darwin
            os: macos-latest
          - target: aarch64-apple-darwin
            os: macos-latest
          - target: x86_64-pc-windows-msvc
            os: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}

      # Every dependency is pure Rust, so a musl build needs no musl-gcc — but
      # cross-linking for aarch64 still needs a linker rustc can drive.
      - name: Use rust-lld for the aarch64 musl cross build
        if: matrix.target == 'aarch64-unknown-linux-musl'
        run: echo "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld" >> "$GITHUB_ENV"

      - name: Build
        run: cargo build --release -p lspgraph-tui --target ${{ matrix.target }}

      - name: Package (unix)
        if: matrix.os != 'windows-latest'
        run: |
          set -euo pipefail
          name="lspgraph-${GITHUB_REF_NAME}-${{ matrix.target }}"
          mkdir "$name"
          cp "target/${{ matrix.target }}/release/lspgraph" "$name/"
          cp README.md LICENSE-MIT LICENSE-APACHE "$name/"
          tar czf "$name.tar.gz" "$name"

      - name: Package (windows)
        if: matrix.os == 'windows-latest'
        shell: pwsh
        run: |
          $name = "lspgraph-$env:GITHUB_REF_NAME-${{ matrix.target }}"
          New-Item -ItemType Directory -Path $name | Out-Null
          Copy-Item "target/${{ matrix.target }}/release/lspgraph.exe" $name
          Copy-Item README.md,LICENSE-MIT,LICENSE-APACHE $name
          Compress-Archive -Path $name -DestinationPath "$name.zip"

      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.target }}
          path: |
            *.tar.gz
            *.zip
          if-no-files-found: error

  publish:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: dist
          merge-multiple: true
      - name: Checksums
        run: |
          cd dist
          sha256sum * > SHA256SUMS
          cat SHA256SUMS
      - uses: softprops/action-gh-release@v2
        with:
          files: dist/*
          generate_release_notes: true
```

- [ ] **Step 2: Lint it**

```bash
"$HOME/go/bin/actionlint" .github/workflows/release.yml
```
Expected: no output.

- [ ] **Step 3: Verify the version guard rejects a mismatched tag**

Test the guard before testing the happy path — a guard that never fires is not a guard.

```bash
git push
git tag v9.9.9
git push origin v9.9.9
gh run watch || true
```
Expected: the `guard` job FAILS with `tag 9.9.9 does not match crate version 0.1.0`, and no build job runs.

Clean up:
```bash
git push --delete origin v9.9.9
git tag -d v9.9.9
```

- [ ] **Step 4: Verify a real release end to end**

The crate version is `0.1.0`, so tag it:

```bash
git tag v0.1.0
git push origin v0.1.0
gh run watch --exit-status
```

Expected: guard passes, all five build jobs pass, and a release appears with five archives plus `SHA256SUMS`.

**The macOS, Windows and aarch64 targets have never been built.** A failure on any of them is an expected outcome, not a surprise. Use `gh run view --log-failed`, fix the workflow, delete and re-push the tag, and repeat. Report every target that needed a fix and what it was.

If `aarch64-unknown-linux-musl` cannot link with `rust-lld`, the fallback is `taiki-e/setup-cross-toolchain-action@v1` — add it for that matrix entry only. Do not silently drop the target.

- [ ] **Step 5: Verify the artifacts actually work**

Download the Linux x86_64 archive and run the binary:

```bash
gh release download v0.1.0 --pattern '*x86_64-unknown-linux-musl*'
tar xzf lspgraph-v0.1.0-x86_64-unknown-linux-musl.tar.gz
./lspgraph-v0.1.0-x86_64-unknown-linux-musl/lspgraph 2>&1 | head -3
sha256sum -c <(gh release download v0.1.0 --pattern 'SHA256SUMS' -O - | grep musl) || true
```
Expected: the binary runs and prints its usage error (`usage: lspgraph <language> <root>`), and the checksum matches. An archive that cannot be run is not a release.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "Add tagged releases with prebuilt binaries for five targets

A version guard runs before anything is built, so a tag that disagrees with
the crate version fails fast rather than publishing a mislabelled binary."
```

---

### Task 5: README — installing, and what Windows does not promise

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: the release artefacts from Task 4.
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Add install instructions**

Add a section above the existing "Try it", using the real asset names produced by Task 4:

```markdown
## Install

Download a prebuilt binary from the [latest release][releases], or build from
source with `cargo build --release`.

```sh
# Linux x86_64, statically linked — runs on any distribution
curl -sL https://github.com/kpanuragh/lspgraph/releases/latest/download/lspgraph-v0.1.0-x86_64-unknown-linux-musl.tar.gz | tar xz
./lspgraph-v0.1.0-x86_64-unknown-linux-musl/lspgraph rust /path/to/project
```

Binaries are published for Linux (x86_64 and aarch64, static), macOS (Intel and
Apple Silicon) and Windows (x86_64). Every release carries a `SHA256SUMS` file.

[releases]: https://github.com/kpanuragh/lspgraph/releases/latest
```

- [ ] **Step 2: State what Windows is and is not**

Add, immediately after that:

```markdown
Windows binaries are built and unit-tested in CI, including every terminal
rendering test. The interactive path — raw mode and the alternate screen on a
real Windows console — is not verified by anything, and four process-lifecycle
tests are Unix-only and do not run there. Treat Windows as untested in practice
until someone confirms it.
```

This matters because the support table in this README already distinguishes
"validated against the real server" from "expected to work, untested". Shipping
a Windows binary while implying Linux-grade confidence would be the first claim
in this project that was not earned.

- [ ] **Step 3: Note that CI exists now**

The README currently says there is no CI pipeline. Replace that with what is
true: pull requests run unit tests on Linux, macOS and Windows plus lint and an
MSRV check; the five-server integration suite runs nightly rather than on every
pull request, because two intermittent failures were observed during development
and never reproduced.

Do NOT describe the integration suite as stable. It is not, and the spec says so.

- [ ] **Step 4: Verify the links and commands**

```bash
grep -o 'https://github.com/[^)]*' README.md | sort -u | while read -r u; do
  printf "  %s -> %s\n" "$u" "$(curl -s -o /dev/null -w '%{http_code}' -L "$u")"
done
```
Expected: `200` for each. A README that links to a 404 is worse than one that
links to nothing.

- [ ] **Step 5: Commit and open the pull request**

```bash
git add README.md
git commit -m "Document installing from a release, and what Windows does not promise"
git push
gh pr create --base master --head ci/add-workflows \
  --title "Add CI and release binaries" \
  --body "Unit tests, lint and an MSRV check on every pull request across Linux, macOS and Windows. The five-server integration suite runs nightly and uploads its full log on failure. Tagged releases build five targets with checksums."
```

---

## Self-Review

**Spec coverage.** §4.1 `ci.yml` → Task 2. §4.2 `integration.yml` → Task 3. §4.3 (scheduled, diagnosable) → Task 3, including Step 4 which proves the artifact upload rather than assuming it. §5 `release.yml`, version guard, five targets, packaging, checksums → Task 4. §6 licences and README → Tasks 1 and 5. §7 Windows honesty → Task 5 Step 2.

**Two spec requirements are met by Task 1 rather than a workflow**, because CI would fail on them immediately: the MSRV claim was false, and the tree is not rustfmt-clean. Gating on either before fixing it would produce a red first run for reasons unrelated to the workflows.

**Placeholder scan.** No "TBD", no "configure appropriately", no "similar to Task N". Every step carries the actual YAML or command. Two steps deliberately instruct STOP-and-report (Task 1 Step 2 if 1.88 is not the floor; Task 4 Step 4 if aarch64 cannot link) — those are escalations, not placeholders.

**Type consistency.** `LSPGRAPH_SERVERS_TOML` is the variable `main.rs` and the integration tests already read; Task 3 sets exactly that name. The archive naming `lspgraph-${GITHUB_REF_NAME}-${target}` in Task 4 is the same string Task 5's README instructions use. `lspgraph-tui` is the package built; `lspgraph` is the binary it produces, which is why Task 4 copies `lspgraph`, not `lspgraph-tui`.

**Known risk carried from the project.** The integration suite has two unreproduced intermittent failures. If Task 3's run fails once and passes on retry, that is the known flake, not a fault in the workflow — report both outcomes and keep the log artifact, which is the first chance anyone has had to capture one.
