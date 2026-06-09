# AGENTS.md — beamtalk-uat

This repo is the **acceptance gate** for Beamtalk: it installs a *released*
toolchain bundle (the artifact a user gets) and drives real projects through it.
The full agent guide lives in [`CLAUDE.md`](./CLAUDE.md) — read it first.

## Failure policy: file a Linear Bug

**This gate exists to catch toolchain regressions, so a failure is a finding —
not something to route around. On *any* failure, file a Linear issue (type
`Bug`) against the Beamtalk (`BT`) project.** This is mandatory for:

- a red UAT scenario (`just uat` / `cargo test`), on any platform;
- a crash, panic, or unexpected behaviour from the released bundle hit *while
  developing* scenarios (build / run / CLI / REPL / MCP / LSP) — even if no
  scenario asserts it yet;
- a contract drift noticed by hand against the installed toolchain.

The default assumption is that **the Beamtalk toolchain must change, not the
scenario** (a scenario encodes a user-facing contract). Do **not** silently work
around a toolchain bug or weaken a scenario to make it pass. If a scenario must
change because the *contract* itself was wrong, say so explicitly in the issue
and the commit.

### How to file

Use the Linear MCP `save_issue` tool (`streamlinear-cli` is **not** installed in
this repo's environment).

- **Team** `BT`, **assignee** `me`, **priority** 3 (raise for data loss or a
  crash on trivial input).
- **Labels:** `Bug` (Issue Type) + one **Item Area**
  (`cli` / `repl` / `runtime` / `codegen` / `parser` / `stdlib` /
  `class-system` / `lsp`) + one **Item Size** (`S` / `M` / `L` / `XL`) +
  `agent-ready` (or `needs-spec` if it needs human clarification).
- **Body:** the failing command / scenario name, the **released version under
  test** (`BEAMTALK_UAT_VERSION` or `tc.version`), expected vs actual, and a
  minimal repro. Note that it was surfaced by beamtalk-uat.
- If you keep or add a scenario that stays red until the fix lands, link the
  issue from that scenario's `expect.toml` comment.

**Example:** [BT-2476](https://linear.app/beamtalk/issue/BT-2476) — `beamtalk new`
scaffolds a project that fails its own `fmt-check`, found via the `cli_*`
scenarios. That is exactly the kind of drift this gate is for.

## Everything else

See [`CLAUDE.md`](./CLAUDE.md) for the gate philosophy, assertion surfaces
(`bunit` / `run` / `cli`), how to add a scenario, the cloud-env setup, and the
CI / release-reporting wiring.
