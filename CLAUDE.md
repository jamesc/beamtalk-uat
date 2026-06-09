# beamtalk-uat — agent guide

This repo is the **acceptance gate** for Beamtalk (Linear epic BT-2253). It is a
*separate consumer* of Beamtalk, modelled on the Swamp gate pattern
(<https://stack72.dev/the-gate-between-our-agent-code-and-our-users/>).

## The one principle that matters

**The released toolchain bundle is the artifact under test — not source.**
The harness installs a published `beamtalk-<version>` release (the same bundle a
user installs) and drives it through real `beamtalk.toml` projects:
`beamtalk build`, `beamtalk run`. We never build Beamtalk from source here.

**Tests are more accurate than the code under test.** When a UAT scenario fails,
the default assumption is that **the Beamtalk toolchain must change, not the
scenario**. A scenario encodes a user-facing contract; if behaviour drifts from
it, that is the regression. Only weaken a scenario when the contract itself was
wrong — and say so explicitly.

## When something fails: file a Linear Bug

This gate exists to **catch toolchain regressions** — so a failure is a finding,
not a chore to route around. **On any failure, file a Linear issue (type `Bug`)
against the Beamtalk (`BT`) project.** This applies to:

- a red UAT scenario (`just uat` / `cargo test`), on any platform;
- a crash, panic, or unexpected behaviour from the released bundle hit *while
  developing* scenarios (build/run/CLI/REPL/MCP/LSP) — even if no scenario
  asserts it yet;
- a contract drift you notice by hand against the installed toolchain.

Do **not** silently work around a toolchain bug or weaken a scenario to make it
pass. If a scenario must change because the *contract* was wrong, say so in the
issue and the commit.

**Filing (use the Linear MCP `save_issue` tool — `streamlinear-cli` is not
installed here):**

- Team `BT`, assignee `me`, priority 3 (raise for data-loss / crash-on-trivial-input).
- Labels: `Bug` (issue type) + one **Item Area** (`cli`, `repl`, `runtime`,
  `codegen`, `parser`, `stdlib`, `class-system`, `lsp`) + one **Item Size**
  (`S`/`M`/`L`/`XL`) + `agent-ready` (or `needs-spec` if under-specified).
- Body: the failing command / scenario name, the **released version under test**
  (`BEAMTALK_UAT_VERSION` / `tc.version`), expected vs actual, and a minimal
  repro. Note it was surfaced by beamtalk-uat.
- If you add or keep a scenario that stays red until the bug is fixed, link the
  issue from the scenario's `expect.toml` comment.

Example: BT-2476 (`beamtalk new` scaffolds a project that fails its own
`fmt-check`) — found via the `cli_*` scenarios, exactly the drift this gate is for.

## Layout

| Path | What |
| --- | --- |
| `src/lib.rs` | Harness: installs/caches a release bundle, exposes a `Toolchain`, stages projects. |
| `src/scenario.rs` | Scenario driver: discovery, `expect.toml` parsing, normalization, build/run/assert. |
| `projects/<name>/` | A real Beamtalk package under test — `beamtalk.toml`, sources in `src/`, optional BUnit tests in `test/`, and an `expect.toml` declaring the assertion surface. |
| `tests/scenarios.rs` | General driver test — auto-discovers and runs all `expect.toml` scenarios. |
| `tests/smoke.rs` | Original smoke test (version check + direct BUnit invocation). |
| `Justfile` | `just uat [version]` runs the suite; `just uat-local <bin>` against a local binary. |

## Assertion surfaces

* **`beamtalk test` (BUnit)** — the default. Deterministic pass/fail via exit
  code; a `test/*.bt` `TestCase` asserts with `self assert: expr equals: value`.
  Preferred because it sidesteps script mode's `halt(0)` truncating stdout.
* **Interactive REPL (via tmux)** — for scenarios that need a *returned value*
  from a live session. `beamtalk run ClassName selector` (script mode) does
  **not** print return values reliably (the node halts before async I/O
  flushes); the REPL evaluates and echoes the value. Drive it by starting
  `beamtalk repl` in a tmux pane, polling for the `>` backend prompt (it appears
  only after the project compiles + connects — *not* at the banner), sending the
  expression, and polling the pane for the result.
* **CLI (`surface = "cli"`)** — drives a `beamtalk <args>` subcommand directly
  and asserts exit code + stdout/stderr **substrings**. This covers the offline
  build/tooling commands that have no REPL op (`new`, `fmt`, `fmt-check`, `lint`,
  `type-coverage`, `build`, `--help`). One scenario per command/behaviour
  (`projects/cli_*`). Note the stdout/stderr split: human-readable diagnostics
  go to **stderr**, machine formats (`--format json`) to **stdout** — assert the
  right stream. Most `cli_*` scenarios are Rust-only and run anywhere; the ones
  that compile (`cli_build`) need Erlang/OTP, so they only go green on the CI
  legs. MCP and LSP surfaces are not yet wired (they ship as separate
  `beamtalk-mcp` / `beamtalk-lsp` binaries in the bundle; LSP runs standalone in
  AST mode, MCP needs a live workspace) — tracked as follow-up.

## Running

```sh
just uat              # latest release
just uat v0.4.0       # a specific release
just uat nightly      # rolling nightly
just uat-local ~/.beamtalk/bin/beamtalk   # an already-installed binary
```

Selection env vars: `BEAMTALK_UAT_VERSION` (`latest`/`nightly`/`vX.Y.Z`) and
`BEAMTALK_UAT_BIN` (skip download, use this binary).

## Requirements

* `gh` (authenticated) — downloads release assets from `jamesc/beamtalk`.
* Erlang/OTP on PATH — `beamtalk build`/`run` need a BEAM runtime.
* `tar` (Unix) / `unzip` (Windows) — archive extraction.

The harness is intentionally **dependency-free** (std + subprocess only) so it
builds offline and can't drift from the toolchain it tests.

## Adding a scenario

The general scenario driver (BT-2450) auto-discovers every `projects/<name>/`
directory that contains an `expect.toml`. No per-scenario Rust test code needed.

1. `cd projects && beamtalk new <name>` to scaffold a real package.
2. Put the behaviour under test in `src/*.bt` (multi-file projects are fine).
3. Add an `expect.toml` in the project root:

   **BUnit scenario** (preferred — deterministic pass/fail):
   ```toml
   surface = "bunit"
   ```
   Then write assertions in `test/*.bt` (`TestCase` using
   `self assert: … equals: …`).

   **Run scenario** (assert stdout / exit code from script mode):
   ```toml
   surface = "run"
   entrypoint = "MyClass mySelector"
   stdout = "expected output"
   exit_code = 0
   ```
   At least one of `stdout` or `exit_code` is required.

   **CLI scenario** (drive a `beamtalk` subcommand directly):
   ```toml
   surface = "cli"
   args = "lint --format json"   # whitespace-split, appended to `beamtalk`
   exit_code = 1                 # optional, defaults to 0
   stdout_contains = "summary"   # optional substring assertion
   stderr_contains = "redundant" # optional substring assertion
   ```
   Runs in the staged (temp-copied) project dir. `args` is required;
   assertions are substring matches. Name CLI scenarios `cli_<command>` and
   keep one command/behaviour per scenario.

4. Done — `just uat` picks it up automatically via `tests/scenarios.rs`.
