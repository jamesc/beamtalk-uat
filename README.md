# beamtalk-uat

The external **acceptance gate** for [Beamtalk](https://github.com/jamesc/beamtalk)
(Linear epic BT-5302, [ADR 0154](https://github.com/jamesc/beamtalk/blob/main/docs/ADR/0154-uat-external-acceptance-gate-for-full-environments.md)).
It follows the Swamp gate pattern:
<https://stack72.dev/the-gate-between-our-agent-code-and-our-users/>.

**The published artifacts are what's under test, never source.** The gate installs
the release bundle a user installs, with only what a user has (their OS, their
Erlang/OTP, the public registry), and drives it. See `CLAUDE.md` for the rules.

> **Migration state (BT-5303, Phase 0).** The harness is a TypeScript project on
> Playwright Test + Zod. The legacy Rust harness (`src/`, `tests/*.rs`, `Cargo.*`,
> `projects/`, `uat.yml`, the sections marked *legacy* below) stays until BT-5305's
> J1/J2 and first contracts are green on all four targets against `latest`; the PR
> that switches `uat.yml` deletes it.

## Quick start

```sh
just install                       # npm ci
just test                          # the harness's tests of itself (hermetic)
just uat                           # the gate vs the latest release (needs gh, Erlang/OTP)
just uat edge                      # ... vs the rolling edge pre-release
just uat v0.4.0                    # ... vs one release
just uat-local ~/.beamtalk/bin/beamtalk   # ... vs a binary you already have
```

The version selector is `latest | edge | nightly | vX.Y.Z | local:<path>`
(`BEAMTALK_UAT_VERSION`). `BEAMTALK_UAT_OTP=<major>` makes the run assert that `erl`
on `PATH` is that major.

## Layout

| Path | What |
| --- | --- |
| `lib/env.ts` | Per-test isolated environment: private `HOME`/`USERPROFILE`, port allocator, unique node names and cookies; strips runner secrets and `BEAMTALK_*`/`RELEASE_*`/`ERL_*` from the child env. |
| `lib/toolchain.ts` | Installs the toolchain under test from a resolve artifact (re-checking its sha256) or a selector; `local:<path>` uses a binary as-is. |
| `lib/selector.ts` | The version selector grammar. |
| `lib/resolve.ts`, `lib/github.ts` | The `resolve` logic (download once, verify, `superseded`) and its GitHub source; `buildMatrix` turns a manifest + `targets.toml` into the workflow matrix. |
| `lib/otp.ts` | OTP major detection/assertion, and the documented per-target OTP install used by J1 (`documentedInstall`). |
| `lib/proc.ts` | `run`/`Proc.spawn`, readiness (`waitForOutput`/`waitForPort`), per-step transcripts, hang guards, process-tree kill, process/epmd scans. |
| `lib/artifacts.ts` | The failure bundle. |
| `lib/index.ts` | The test API: `test`, `expect`, and the per-test `uat` handle (`uat.bt`, `uat.run`, `uat.spawn`, `uat.step`). |
| `schemas/` | Zod: the dispatch payloads, the resolve manifest, the installed-toolchain record. |
| `targets.toml` | Targets, runner images and the per-target OTP legs (owner-gated). |
| `scripts/resolve.ts` | Entry point of the `resolve` job. `scripts/prepare-toolchain.ts` installs the toolchain before `playwright test`. |
| `tests/harness/` | Tests **of the harness** (project `harness`): resolver, matrix, selector, proc, env, and a nested run that proves the failure bundle and leak checks. |
| `tests/smoke/` | A plumbing smoke against the real bundle (project `smoke`). Journeys, contracts and adversarial tests arrive in later phases (ADR § 5). |
| `.github/workflows/gate.yml` | The gate: `resolve` -> `gate` (target x OTP x shard) -> `result`. |

## How a run works

1. **`resolve`** takes the trigger (a `beamtalk-artifact` payload, a legacy
   `beamtalk-release` payload, or a selector). It downloads every asset **once**,
   checks each sha256 (the payload's `assets` map; for the legacy type the published
   `.sha256` files) and uploads one workflow artifact: `assets/` + `manifest.json`.
2. A hash that no longer matches means the rolling release (`edge`/`nightly`) was
   replaced mid-flight. The run ends **`superseded`**: cancelled, nothing filed.
   A mismatch on a tagged release is a real integrity failure. Concurrency groups
   cancel the older run for `edge` and `nightly`, never for a release.
3. **`gate`** runs one job per leg in `targets.toml`: `linux-x86_64`
   (`ubuntu-24.04`, OTP min and max), `macos-arm64` (`macos-15`, max),
   `macos-x86_64` (`macos-15-intel`, min), `windows-x86_64` (`windows-2022`, max).
   The OTP majors come from the dispatch payload (`otp-support.toml`), so a window
   bump needs no edit here. Each leg installs the bundle from the run's artifact
   and never touches the release again.
4. Every test records a **transcript per step**. A red test attaches a **failure
   bundle**: transcripts, workspace and release logs, `erl_crash.dump`,
   `epmd -names`, the process tree, `beamtalk --version` and `doctor`, and a one-line
   repro. **Teardown is checked**: a process or epmd registration left behind is a
   red step. A step has a hang guard (default 60s) that names the step.

## Writing a test

```ts
import { expect, test } from '../../lib/index.ts';

test('the bundle reports a version', { tag: '@max-otp' }, async ({ uat }) => {
  await uat.step('beamtalk --version', async () => {
    const r = await uat.bt(['--version']);
    expect(r.code, r.stderr).toBe(0);
  });
});
```

Stop every long-lived process you `uat.spawn` before the test ends. Tag a test
`@max-otp` to include it in linux's max-major leg.

## Workflow triggers

`gate.yml` runs on `repository_dispatch` `beamtalk-artifact` (the ADR § 3 payload),
`repository_dispatch` `beamtalk-release` (legacy `{version, tag}`, accepted until the
release after Phase 0) and `workflow_dispatch` with a selector. `ci.yml` runs the
harness self-tests (Linux and Windows) and a smoke against `edge` on every PR.

---

# Legacy Rust harness (removed at switch-over)

## Requirements

- `gh` (authenticated), Erlang/OTP on PATH, and `cargo` + `just`.

In a fresh cloud session (Claude Code on the web), run `scripts/setup-cloud.sh`
to install the missing pieces — it provisions **Erlang/OTP** (pinned by
`.tool-versions`, installed via `mise` — the same mechanism CI's `setup-beam`
and the beamtalk repo use, so the OTP version never drifts from what the
released bundle is built against) and **just** (Rust/cargo, `gh`, `tar`/`unzip`
are assumed present) and skips anything already installed. A `SessionStart`
hook (`.claude/settings.json`) runs it automatically on the first session,
guarded by a marker file. The script deliberately installs
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
