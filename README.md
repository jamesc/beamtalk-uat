# beamtalk-uat

Acceptance-test **gate** for [Beamtalk](https://github.com/jamesc/beamtalk).

This repo is a *separate consumer* of Beamtalk: it installs a **released
toolchain bundle** — the exact artifact a user gets — and runs real Beamtalk
packages (created with `beamtalk new`) through the toolchain, asserting their
behaviour. It exists to cover end-to-end, full-project scenarios that the
in-repo per-file/single-file harnesses can't reach (Linear epic BT-2253). The
design follows the Swamp gate pattern:
<https://stack72.dev/the-gate-between-our-agent-code-and-our-users/>.

The released bundle is the thing under test — nothing here builds Beamtalk from
source.

## Quick start

```sh
just uat              # install the latest release and run the suite
just uat v0.4.0       # pin a specific release
just uat nightly      # the rolling nightly build
```

Local development against a binary you already have:

```sh
just uat-local ~/.beamtalk/bin/beamtalk
```

## How it works

1. The harness (`src/lib.rs`) downloads the platform's release asset from
   `jamesc/beamtalk` via `gh release download`, verifies its checksum, and
   extracts it to a cached install — once per run, reused across scenarios.
2. The scenario driver (`src/scenario.rs`, `tests/scenarios.rs`) auto-discovers
   every `projects/<name>/` directory with an `expect.toml`, stages it to a temp
   dir, and exercises it through the installed toolchain.

See `CLAUDE.md` for the assertion surfaces (BUnit vs. the interactive REPL for
value-returning scenarios) and how to add a scenario.

## Scenario layout

Each scenario is a self-contained Beamtalk project under `projects/<name>/`:

```
projects/
  smoke/
    beamtalk.toml      # standard Beamtalk package manifest
    expect.toml        # declares the assertion surface + expected outcome
    src/
      Smoke.bt         # source under test (multi-file projects are supported)
    test/
      SmokeTest.bt     # BUnit assertions (for bunit surface)
```

### `expect.toml` format

**BUnit surface** — runs `beamtalk test` and asserts all tests pass:

```toml
surface = "bunit"
```

**Run surface** — runs `beamtalk run <Class> <selector>` and asserts stdout /
exit code:

```toml
surface = "run"
entrypoint = "MyClass mySelector"
stdout = "expected output"    # compared after normalization
exit_code = 0                 # optional, defaults to not checked
```

At least one of `stdout` or `exit_code` is required for `run` scenarios.

Output normalization trims whitespace, collapses internal runs to single spaces,
and replaces Erlang PIDs (`<0.123.0>`) with `<pid>` so assertions aren't fragile.

**CLI surface** — runs an arbitrary `beamtalk <args>` subcommand in the staged
project dir and asserts exit code plus stdout/stderr **substrings**:

```toml
surface = "cli"
args = "lint --format json"   # appended to `beamtalk`, whitespace-split
exit_code = 1                 # optional, defaults to 0 (success)
stdout_contains = "summary"   # optional substring assertion
stderr_contains = "redundant" # optional substring assertion
```

This is the surface for the offline build/tooling commands with no REPL op —
`new`, `fmt`, `fmt-check`, `lint`, `type-coverage`, `build`, `--help`, … One
scenario per command/behaviour (`projects/cli_*`). Substring (not exact)
matching is used because CLI output embeds absolute paths and version strings
that vary per environment. The staged project is a throwaway temp copy, so
commands that mutate the tree (`fmt`, `new`) are isolated. Most `cli_*`
scenarios are Rust-only and run anywhere; `cli_build` needs Erlang/OTP (the CI
legs), like the `run`/`bunit` surfaces.

**LSP surface** — drives the bundled `beamtalk-lsp` server over stdio JSON-RPC
(a hand-rolled, dependency-free client in `src/lsp.rs`) and asserts a
**substring** of one request's response:

```toml
surface = "lsp"
method = "textDocument/hover"   # the LSP request to send
source = "src/LspHover.bt"      # project-relative file to open
line = 4                        # 0-based cursor (position requests only)
character = 18                  # 0-based cursor
response_contains = "Extends:"  # substring asserted in the response
```

The server runs standalone in its in-process **AST mode** (no workspace, no
BEAM), so `lsp_*` scenarios cover editor capabilities (`documentSymbol`,
`hover`, `completion`, `definition`, …) and go green on **every** platform leg,
not just the ones with Erlang. `documentSymbol`/`formatting` need no
`line`/`character`; position requests do. Assert on value substrings (`"self"`,
`Extends:`, a filename) that don't depend on the server's JSON key spacing. One
scenario per capability (`projects/lsp_*`).

**MCP surface** — drives the bundled `beamtalk-mcp` server over stdio JSON-RPC
(a hand-rolled, dependency-free client in `src/mcp.rs`; **newline-delimited**,
unlike LSP's `Content-Length` framing), calls one tool, and asserts a
**substring** of the result:

```toml
surface = "mcp"
tool = "evaluate"          # the MCP tool to call
code = "1 + 1"             # shortcut → {"code": "1 + 1"} arguments
# arguments = "{}"         # …or raw JSON arguments for other tools
response_contains = "2"    # substring asserted in the tool result
```

`beamtalk-mcp` is launched with `--start`, which boots a live `beamtalk repl`
**workspace** from the staged project — so `mcp_*` scenarios need a BEAM runtime
(the CI legs), the project must compile, and the harness prepends the bundle
`bin/` to `PATH` (since `--start` shells out to `beamtalk`). The `code` key is a
shortcut for `evaluate`-style tools; other tools take raw `arguments` JSON. One
scenario per tool/behaviour (`projects/mcp_*`).

## Requirements

- `gh` (authenticated), Erlang/OTP on PATH, and `cargo` + `just`.

In a fresh cloud session (Claude Code on the web), run `scripts/setup-cloud.sh`
to install the missing pieces — it provisions **Erlang/OTP 27** and **just**
(Rust/cargo, `gh`, `tar`/`unzip` are assumed present) and skips anything already
installed. A `SessionStart` hook (`.claude/settings.json`) runs it automatically
on the first session, guarded by a marker file. The script deliberately installs
no Elixir/Mix toolchain: the LiveView IDE isn't part of the released bundle UAT
drives, and the gate-consistent way to test it later is a self-contained
`mix release` OTP tarball that needs no host Elixir.

## CI

Two workflows, with distinct jobs:

- **`.github/workflows/ci.yml`** — PR CI for the *harness itself*, on every
  `pull_request` and push to `main`. A hermetic `check` job (fmt, clippy, the
  lib unit tests, a build, shellcheck on the setup scripts — no network/Erlang)
  and an `e2e` job that installs Erlang/OTP and runs the full `#[ignore]d`
  scenario suite against **edge** on one Linux leg, so harness changes meet a
  real toolchain before merge. Edge (not `latest`) because the harness is
  developed against the toolchain tip — `edge` is the rolling Linux pre-release
  beamtalk republishes on every merge to `main` (`edge.yml`), so it always
  carries features that have landed but aren't in a tagged release yet, with no
  nightly-cadence lag. Validating a *specific* released version is the release
  gate's job (below).
- **`.github/workflows/uat.yml`** — the release acceptance gate (below).

The `uat.yml` gate installs a released bundle and runs the full scenario suite
across the release platforms (Linux, macOS x86_64 / arm64, Windows). It runs on:

- **`repository_dispatch`** (event-type `beamtalk-release`) — fired by
  jamesc/beamtalk's `release.yml` (BT-2449) after a release is published; tests
  that exact version (read from `client_payload.version`).
- **`workflow_dispatch`** — manual run with a `version` input
  (`latest` / `nightly` / `vX.Y.Z`).
- **`schedule`** — nightly cron against the rolling `nightly` release.

Job status reflects pass/fail and failed scenarios are surfaced in the job
summary. See `CLAUDE.md` for the gate philosophy and how to add scenarios.

### Reporting back onto the beamtalk release (BT-2453)

On the **`repository_dispatch`** path — i.e. a real published release — a final
`report` job posts the UAT outcome as a **GitHub commit status** (context
`uat-gate`) on the commit the released tag points at, in `jamesc/beamtalk`. The
result is then visible from the originating release/tag, and the status's
`target_url` links straight to this UAT run, so a failure is one click away from
the failing logs.

This is **non-blocking / reporting mode**: the status is informational and does
not (yet) gate the release. The job:

- fires **only** on `repository_dispatch` (manual `workflow_dispatch` and
  nightly `schedule` runs post nothing — they don't map to a tagged release);
- runs with `if: always()` and derives pass/fail from the matrix job result, so
  a failing or cancelled leg reports `failure`, never a false `success`;
- resolves the dispatch payload's `tag` (e.g. `v0.4.0`) → commit SHA via the
  GitHub API, then `POST`s `/repos/jamesc/beamtalk/statuses/{sha}`.

It needs **`secrets.BEAMTALK_STATUS_TOKEN`** — a PAT / fine-grained token scoped
to `jamesc/beamtalk` with *commit statuses: write* (classic scope `repo:status`),
provisioned separately as a repo secret. This is **distinct** from beamtalk's
`UAT_DISPATCH_TOKEN` (BT-2449): that token writes the dispatch *into* this repo;
`BEAMTALK_STATUS_TOKEN` writes the status *into* beamtalk. `GITHUB_TOKEN` can't
be used because it has no write access to another repository.

### Flipping to a blocking promotion gate (follow-up)

Today the gate **reports**; it does not block. The path to a true
[promote-on-pass gate](https://stack72.dev/the-gate-between-our-agent-code-and-our-users/)
— where a failing UAT run prevents the release from reaching users — is:

1. **Publish as a pre-release.** beamtalk's `release.yml` tags and uploads the
   bundle but marks the GitHub release `prerelease: true`, and `install.sh`'s
   `latest` pointer is **not** moved to it yet. Users on `latest` keep the prior
   known-good version.
2. **Dispatch UAT.** The existing `repository_dispatch` (BT-2449) fires; this
   suite installs the pre-release bundle and runs every scenario across all
   platforms — exactly as now.
3. **Promote on pass.** When the gate is green, a promotion step (in beamtalk,
   keyed off the `uat-gate` commit status this job posts) flips the release from
   pre-release → full release **and** advances the `install.sh` `latest`
   pointer to the new version.
4. **Failure blocks promotion.** A red `uat-gate` status leaves the release as a
   pre-release and `latest` unchanged, so the regression never reaches users on
   the default install path; the failing run is linked from the status for
   triage.

Flipping requires no change to the assertion surface — only wiring the existing
`uat-gate` status into beamtalk's release flow as a required, promotion-gating
check. It is intentionally deferred until the reporting signal is trusted.
