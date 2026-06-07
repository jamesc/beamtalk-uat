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
2. Each project under `projects/<name>/` is a real Beamtalk package (`beamtalk
   new`), with behaviour in `src/` and BUnit assertions in `test/`. A test
   stages it to a temp dir and runs `beamtalk test`, asserting it passes.

See `CLAUDE.md` for the assertion surfaces (BUnit vs. the interactive REPL for
value-returning scenarios) and how to add a scenario.

## Requirements

- `gh` (authenticated), Erlang/OTP on PATH, and `cargo` + `just`.

## CI

In CI the suite is triggered against a freshly published release and reports
pass/fail (the cross-repo dispatch + reporting wiring lands in BT-2449 / BT-2451
/ BT-2453). See `CLAUDE.md` for the gate philosophy and how to add scenarios.
