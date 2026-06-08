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

4. Done — `just uat` picks it up automatically via `tests/scenarios.rs`.
