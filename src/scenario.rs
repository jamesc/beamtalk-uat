// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

//! Scenario discovery, parsing, and assertion driver (BT-2450, BT-2480).
//!
//! A `projects/<name>/` directory is a real Beamtalk package (a **fixture**):
//! a valid `beamtalk.toml`, sources in `src/`, optional tests in `test/`, and an
//! `expect.toml`. One fixture declares **one or more scenarios**, each of **one
//! or more steps** — so the scenario count is decoupled from the project count
//! (no more one-package-per-assertion sprawl).
//!
//! ## `expect.toml` format
//!
//! Three shapes are accepted, all parsed by `toml` + `serde`:
//!
//! ```toml
//! # -- Flat form (one scenario, one step): the common case --
//! surface = "cli"
//! args = "lint --format json"
//! exit_code = 1
//! stdout_contains = "severity"
//!
//! # -- Fan-out form ([[scenario]]): many independent scenarios share one
//! #    fixture package. Each gets a `name`; the display name is `<dir>/<name>`. --
//! [[scenario]]
//! name = "hover"
//! surface = "lsp"
//! method = "textDocument/hover"
//! source = "src/Counter.bt"
//! line = 4
//! character = 18
//! response_contains = "Extends:"
//!
//! [[scenario]]
//! name = "document_symbol"
//! surface = "lsp"
//! method = "textDocument/documentSymbol"
//! source = "src/Counter.bt"
//! response_contains = "increment:"
//!
//! # -- Sequence form ([[step]]): ordered steps against ONE live session, so a
//! #    later step observes an earlier step's side effect. Steps are only valid
//! #    on session-backed surfaces (`lsp`, `mcp`). Per-step `response_contains`
//! #    is optional in a multi-step scenario (a side-effect step need only not
//! #    error). Combine with [[scenario]] via [[scenario.step]]. --
//! surface = "mcp"
//! [[step]]
//! tool = "evaluate"
//! code = "Counter spawn"       # side effect: create an actor
//! [[step]]
//! tool = "workspace_actors"    # observe it
//! response_contains = "Counter"
//! ```
//!
//! Per-surface fields:
//!
//! * `bunit` — no fields; runs `beamtalk test` and asserts a passing run.
//! * `run` — `entrypoint = "Class selector"`, plus at least one of `stdout`
//!   (exact after normalization) / `exit_code`.
//! * `cli` — `args` (whitespace-split, appended to `beamtalk`), optional
//!   `exit_code` (default 0), `stdout_contains`, `stderr_contains`, `setup`
//!   (a `beamtalk` cmd run first, must exit 0), `cwd` (staged subdir to run in).
//! * `lsp` — `method`, `source`, `response_contains`; position requests also
//!   need `line` + `character`.
//! * `mcp` — `tool`, `response_contains` (required for a single-step scenario),
//!   and either `code` (the `{"code": …}` shortcut) or raw `arguments` JSON.
//!
//! `cli`/`lsp`/`mcp` assertions are **substrings**; `run`'s `stdout` is exact
//! after normalization (CLI output embeds paths/versions that vary per host).
//!
//! ## Output normalization
//!
//! Before comparing `stdout`, both expected and actual values are normalized:
//! leading/trailing whitespace trimmed, internal runs of whitespace collapsed
//! to a single space, and Erlang PIDs (`<0.123.0>`) replaced with `<pid>`.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Toolchain;

// ── Expectation types ───────────────────────────────────────────────────────

/// The assertion surface a scenario exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    /// Run `beamtalk test` — deterministic pass/fail via BUnit.
    Bunit,
    /// Run `beamtalk run <Class> <selector>` and assert stdout / exit code.
    Run,
    /// Run an arbitrary `beamtalk <args>` subcommand and assert exit code /
    /// stdout & stderr substrings. The surface for offline build/tooling
    /// commands (`new`, `fmt`, `lint`, `type-coverage`, `build`, …).
    Cli,
    /// Drive the bundled `beamtalk-lsp` server over stdio JSON-RPC and assert a
    /// substring of one request's response. Runs standalone (AST mode, no BEAM),
    /// so it covers editor capabilities (`documentSymbol`, `hover`, `completion`,
    /// `definition`, …) on any platform.
    Lsp,
    /// Drive the bundled `beamtalk-mcp` server over stdio JSON-RPC, call one
    /// tool, and assert a substring of the result. `--start` spawns a live
    /// `beamtalk repl` workspace, so this needs a BEAM runtime (the CI legs).
    Mcp,
}

impl Surface {
    /// Whether a scenario on this surface keeps one live session alive across
    /// steps — and therefore may carry an ordered `[[step]]` sequence. `cli` /
    /// `run` / `bunit` spawn a fresh process per action with no shared state.
    fn is_session_backed(self) -> bool {
        matches!(self, Surface::Lsp | Surface::Mcp)
    }
}

/// One action within a scenario. For a single-step scenario this is the whole
/// scenario; for a `[[step]]` sequence each entry is one action against the
/// shared session. Which fields are meaningful depends on the scenario surface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Step {
    /// `Class selector` entrypoint (`run` surface).
    pub entrypoint: Option<String>,
    /// Arguments appended to `beamtalk`, whitespace-split (`cli` surface).
    pub args: Option<String>,
    /// Optional `beamtalk` subcommand run *before* `args` (`cli` surface): lets
    /// a scenario scaffold state — e.g. `new <pkg>` — that the asserted command
    /// then runs against. Must exit 0.
    pub setup: Option<String>,
    /// Optional working directory (relative to the staged project dir) the
    /// asserted `args` run in (`cli` surface). Used with `setup`.
    pub cwd: Option<String>,
    /// Expected (normalized) stdout, compared *exactly* (`run` surface).
    pub stdout: Option<String>,
    /// Substring expected in stdout (`cli` surface).
    pub stdout_contains: Option<String>,
    /// Substring expected in stderr (`cli` surface).
    pub stderr_contains: Option<String>,
    /// Expected process exit code. For `cli` an absent value means "expect
    /// success" (0); for `run` it is only asserted when present.
    pub exit_code: Option<i32>,
    /// LSP request method (`lsp` surface), e.g. `"textDocument/documentSymbol"`.
    #[serde(rename = "method")]
    pub lsp_method: Option<String>,
    /// Project-relative path of the source file to open (`lsp` surface).
    pub source: Option<String>,
    /// 0-based cursor line for position-based LSP requests (`lsp` surface).
    pub line: Option<u32>,
    /// 0-based cursor character for position-based LSP requests (`lsp` surface).
    pub character: Option<u32>,
    /// Substring expected in the LSP/MCP response (`lsp` / `mcp` surfaces).
    pub response_contains: Option<String>,
    /// MCP tool name to call (`mcp` surface), e.g. `"evaluate"`.
    pub tool: Option<String>,
    /// Convenience for evaluate-style tools (`mcp` surface): the value becomes
    /// `{"code": "<code>"}` arguments.
    pub code: Option<String>,
    /// Raw JSON object passed as the tool's `arguments` (`mcp` surface); used for
    /// tools other than the `code` shortcut. Defaults to `{}`.
    pub arguments: Option<String>,
}

impl Step {
    /// Whether every field is unset — used to reject mixing top-level step
    /// fields with an explicit `[[step]]` sequence. Compared against `default()`
    /// so a newly added field is covered automatically.
    fn is_empty(&self) -> bool {
        *self == Step::default()
    }
}

/// A discovered scenario ready to be driven. One fixture directory can yield
/// several of these (fan-out); each runs against its own staged copy.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// Unique display name (e.g. `lsp` or `lsp/hover`).
    pub name: String,
    /// The `projects/<dir_name>` directory this scenario stages from. Distinct
    /// from `name`, which may be `<dir_name>/<scenario-name>` under fan-out.
    pub dir_name: String,
    /// Path to the project directory (inside the repo, not yet staged).
    pub project_dir: PathBuf,
    /// Which surface every step exercises (one surface per scenario — a stepped
    /// scenario shares a single session, so it cannot mix surfaces).
    pub surface: Surface,
    /// Ordered steps. Always at least one; >1 only on session-backed surfaces.
    pub steps: Vec<Step>,
}

// ── Raw (serde) wire types ──────────────────────────────────────────────────

/// A scenario as written in `expect.toml`: a surface, an optional name, the
/// inline single-step action fields (flattened), and an optional `[[step]]`
/// sequence.
#[derive(Debug, Clone, Deserialize)]
struct RawScenario {
    name: Option<String>,
    surface: Surface,
    #[serde(flatten)]
    action: Step,
    #[serde(default)]
    step: Vec<Step>,
}

/// The `[[scenario]]` (fan-out) form of an `expect.toml`. `deny_unknown_fields`
/// rejects a file that mixes top-level flat fields (e.g. `surface = …`) with a
/// `[[scenario]]` array — otherwise the flat scenario would be silently dropped.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioFile {
    scenario: Vec<RawScenario>,
}

// ── Discovery ───────────────────────────────────────────────────────────────

/// Discover all scenarios under `projects/` (one or more per `expect.toml`).
///
/// Returns an alphabetically sorted list. Projects without `expect.toml` are
/// silently skipped — they may be fixture data for other tests.
pub fn discover(projects_dir: &Path) -> Result<Vec<Scenario>, String> {
    let entries = std::fs::read_dir(projects_dir)
        .map_err(|e| format!("read_dir {}: {e}", projects_dir.display()))?;
    let mut scenarios = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read_dir entry: {e}"))?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let expect_path = dir.join("expect.toml");
        if !expect_path.exists() {
            continue;
        }
        let dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let text = std::fs::read_to_string(&expect_path)
            .map_err(|e| format!("read {}: {e}", expect_path.display()))?;
        let parsed = parse_expect_text(&text, &dir_name, &dir)?;
        scenarios.extend(parsed);
    }
    scenarios.sort_by(|a, b| a.name.cmp(&b.name));

    // Guard against duplicate display names (e.g. two `[[scenario]]` entries
    // with the same `name`), which would make failures ambiguous.
    for pair in scenarios.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(format!("duplicate scenario name `{}`", pair[0].name));
        }
    }
    Ok(scenarios)
}

/// Parse one `expect.toml`'s text into its scenarios.
fn parse_expect_text(
    text: &str,
    dir_name: &str,
    project_dir: &Path,
) -> Result<Vec<Scenario>, String> {
    let value: toml::Value =
        toml::from_str(text).map_err(|e| format!("scenario `{dir_name}`: invalid TOML: {e}"))?;

    // Reject typo'd keys before deserializing: `#[serde(flatten)]` on
    // `RawScenario.action` is structurally incompatible with
    // `deny_unknown_fields`, so a misspelled optional key (`stdout_contians`)
    // would silently drop to `None` and the scenario would assert nothing
    // (BT-2481). Validate the raw key set explicitly instead.
    reject_unknown_keys(&value, dir_name)?;

    // `[[scenario]]` (fan-out) form vs. flat (single-scenario) form.
    let is_array = value.get("scenario").is_some();
    let raws: Vec<RawScenario> = if is_array {
        let file: ScenarioFile = value
            .try_into()
            .map_err(|e| format!("scenario `{dir_name}`: {e}"))?;
        if file.scenario.is_empty() {
            return Err(format!(
                "scenario `{dir_name}`: `[[scenario]]` array is empty"
            ));
        }
        file.scenario
    } else {
        vec![value
            .try_into()
            .map_err(|e| format!("scenario `{dir_name}`: {e}"))?]
    };

    let mut out = Vec::with_capacity(raws.len());
    for (i, raw) in raws.into_iter().enumerate() {
        // Display name: `<dir>` for the flat form, `<dir>/<name-or-index>` for
        // fan-out so each scenario is addressable.
        let display = match (&raw.name, is_array) {
            (Some(n), _) => format!("{dir_name}/{n}"),
            (None, true) => format!("{dir_name}/{i}"),
            (None, false) => dir_name.to_string(),
        };

        // Resolve steps: an explicit `[[step]]` sequence, or the inline action
        // as a single step.
        let steps = if raw.step.is_empty() {
            vec![raw.action]
        } else {
            if !raw.action.is_empty() {
                return Err(format!(
                    "scenario `{display}`: set either top-level step fields or `[[step]]`, not both"
                ));
            }
            if !raw.surface.is_session_backed() {
                return Err(format!(
                    "scenario `{display}`: `[[step]]` sequences require a session-backed surface \
                     (`lsp` or `mcp`); `{:?}` spawns a fresh process per step with no shared state",
                    raw.surface
                ));
            }
            raw.step
        };

        let multi = steps.len() > 1;
        for (si, step) in steps.iter().enumerate() {
            validate_step(raw.surface, step, multi)
                .map_err(|e| format!("scenario `{display}` step {}: {e}", si + 1))?;
        }

        out.push(Scenario {
            name: display,
            dir_name: dir_name.to_string(),
            project_dir: project_dir.to_path_buf(),
            surface: raw.surface,
            steps,
        });
    }
    Ok(out)
}

/// Validate one step's fields against its surface. `multi` is true when the step
/// is part of a multi-step sequence (which relaxes some single-step requirements).
fn validate_step(surface: Surface, step: &Step, multi: bool) -> Result<(), String> {
    // Reject fields that belong to a different surface — they'd be silently
    // ignored at runtime, quietly weakening the scenario. `expect.toml` fields
    // are surface-specific, so a stray key is an authoring error.
    let stray = unexpected_fields(surface, step);
    if !stray.is_empty() {
        return Err(format!(
            "`{}` surface does not use field(s): {}",
            surface_name(surface),
            stray.join(", ")
        ));
    }

    match surface {
        Surface::Bunit => {}
        Surface::Run => {
            if step.entrypoint.is_none() {
                return Err("`run` requires an `entrypoint`".to_string());
            }
            if step.stdout.is_none() && step.exit_code.is_none() {
                return Err("`run` requires at least one of `stdout` or `exit_code`".to_string());
            }
        }
        Surface::Cli => {
            if step.args.is_none() {
                return Err("`cli` requires an `args` key".to_string());
            }
        }
        Surface::Lsp => {
            let Some(method) = step.lsp_method.as_deref() else {
                return Err("`lsp` requires a `method` key".to_string());
            };
            if step.source.is_none() {
                return Err("`lsp` requires a `source` key".to_string());
            }
            // A lone step must assert something; in a sequence a side-effect
            // step may omit `response_contains` (it only needs to not error).
            if !multi && step.response_contains.is_none() {
                return Err("`lsp` requires a `response_contains` key".to_string());
            }
            if lsp_needs_position(method) && (step.line.is_none() || step.character.is_none()) {
                return Err(format!(
                    "lsp method `{method}` requires `line` and `character`"
                ));
            }
        }
        Surface::Mcp => {
            if step.tool.is_none() {
                return Err("`mcp` requires a `tool` key".to_string());
            }
            // A lone step must assert something; in a sequence a side-effect
            // step may omit `response_contains` (it only needs to not error).
            if !multi && step.response_contains.is_none() {
                return Err("`mcp` requires a `response_contains` key".to_string());
            }
            if step.code.is_some() && step.arguments.is_some() {
                return Err("`mcp`: use either `code` or `arguments`, not both".to_string());
            }
        }
    }
    Ok(())
}

/// Every TOML key a single `Step` (action) may carry. Kept in lockstep with the
/// `Step` struct's `expect.toml` field names (`method` maps to `lsp_method`).
/// Used to reject typo'd keys that `#[serde(flatten)]` would otherwise drop.
const STEP_FIELDS: &[&str] = &[
    "entrypoint",
    "args",
    "setup",
    "cwd",
    "stdout",
    "stdout_contains",
    "stderr_contains",
    "exit_code",
    "method",
    "source",
    "line",
    "character",
    "response_contains",
    "tool",
    "code",
    "arguments",
];

/// Reject any `expect.toml` key that no scenario- or step-level field claims.
///
/// `#[serde(flatten)]` cannot be combined with `#[serde(deny_unknown_fields)]`,
/// so serde silently drops an unknown key to `None`. For an *optional* field
/// (`stdout_contains`, a step's `response_contains`) that means a typo'd
/// assertion never runs and the scenario goes green asserting nothing — exactly
/// the failure mode this gate exists to prevent (BT-2481). We walk the raw
/// `toml::Value` and reject stray keys before deserializing.
fn reject_unknown_keys(value: &toml::Value, dir_name: &str) -> Result<(), String> {
    let table = value
        .as_table()
        .ok_or_else(|| format!("scenario `{dir_name}`: expect.toml must be a table"))?;

    if let Some(scenarios) = table.get("scenario") {
        // Fan-out form: `scenario` is the only legal top-level key (mixing flat
        // fields with the array is already rejected by `deny_unknown_fields` on
        // `ScenarioFile`, but catch it here too with a clearer message).
        for key in table.keys() {
            if key != "scenario" {
                return Err(format!(
                    "scenario `{dir_name}`: unknown top-level key `{key}` \
                     (a `[[scenario]]` file may only contain the `scenario` array)"
                ));
            }
        }
        let arr = scenarios
            .as_array()
            .ok_or_else(|| format!("scenario `{dir_name}`: `scenario` must be an array"))?;
        for scn in arr {
            let t = scn.as_table().ok_or_else(|| {
                format!("scenario `{dir_name}`: each `[[scenario]]` must be a table")
            })?;
            check_scenario_table(t, dir_name)?;
        }
    } else {
        // Flat form: the top-level table is itself a single scenario.
        check_scenario_table(table, dir_name)?;
    }
    Ok(())
}

/// Validate the keys of one scenario table — its scenario-level keys (`name`,
/// `surface`, `step`, plus the flattened `Step` action fields) and the keys of
/// each `[[step]]` sub-table.
fn check_scenario_table(table: &toml::value::Table, dir_name: &str) -> Result<(), String> {
    for key in table.keys() {
        let known = matches!(key.as_str(), "name" | "surface" | "step")
            || STEP_FIELDS.contains(&key.as_str());
        if !known {
            return Err(format!("scenario `{dir_name}`: unknown key `{key}`"));
        }
    }
    if let Some(steps) = table.get("step") {
        let arr = steps
            .as_array()
            .ok_or_else(|| format!("scenario `{dir_name}`: `step` must be an array"))?;
        for step in arr {
            let t = step
                .as_table()
                .ok_or_else(|| format!("scenario `{dir_name}`: each `[[step]]` must be a table"))?;
            for key in t.keys() {
                if !STEP_FIELDS.contains(&key.as_str()) {
                    return Err(format!("scenario `{dir_name}`: unknown step key `{key}`"));
                }
            }
        }
    }
    Ok(())
}

/// The `expect.toml` field names set on `step` that do not belong to `surface`.
/// Keyed by the TOML field name so error messages match what the author wrote.
fn unexpected_fields(surface: Surface, step: &Step) -> Vec<&'static str> {
    let present: [(&str, bool); 16] = [
        ("entrypoint", step.entrypoint.is_some()),
        ("args", step.args.is_some()),
        ("setup", step.setup.is_some()),
        ("cwd", step.cwd.is_some()),
        ("stdout", step.stdout.is_some()),
        ("stdout_contains", step.stdout_contains.is_some()),
        ("stderr_contains", step.stderr_contains.is_some()),
        ("exit_code", step.exit_code.is_some()),
        ("method", step.lsp_method.is_some()),
        ("source", step.source.is_some()),
        ("line", step.line.is_some()),
        ("character", step.character.is_some()),
        ("response_contains", step.response_contains.is_some()),
        ("tool", step.tool.is_some()),
        ("code", step.code.is_some()),
        ("arguments", step.arguments.is_some()),
    ];
    let allowed: &[&str] = match surface {
        Surface::Bunit => &[],
        Surface::Run => &["entrypoint", "stdout", "exit_code"],
        Surface::Cli => &[
            "args",
            "setup",
            "cwd",
            "stdout_contains",
            "stderr_contains",
            "exit_code",
        ],
        Surface::Lsp => &["method", "source", "line", "character", "response_contains"],
        Surface::Mcp => &["tool", "code", "arguments", "response_contains"],
    };
    present
        .into_iter()
        .filter(|(name, set)| *set && !allowed.contains(name))
        .map(|(name, _)| name)
        .collect()
}

/// Lowercase surface name as written in `expect.toml`.
fn surface_name(surface: Surface) -> &'static str {
    match surface {
        Surface::Bunit => "bunit",
        Surface::Run => "run",
        Surface::Cli => "cli",
        Surface::Lsp => "lsp",
        Surface::Mcp => "mcp",
    }
}

/// Whether an LSP method takes a `position` (cursor) in its params. Outline /
/// whole-document requests (`documentSymbol`, `formatting`) do not.
fn lsp_needs_position(method: &str) -> bool {
    matches!(
        method,
        "textDocument/hover"
            | "textDocument/definition"
            | "textDocument/completion"
            | "textDocument/references"
            | "textDocument/implementation"
            | "textDocument/signatureHelp"
            | "textDocument/prepareCallHierarchy"
            | "textDocument/prepareTypeHierarchy"
    )
}

// ── Output normalization ────────────────────────────────────────────────────

/// Normalize a command's textual output for comparison.
///
/// * Trims leading/trailing whitespace.
/// * Collapses runs of internal whitespace to a single space.
/// * Replaces Erlang PIDs (`<0.123.0>`) with `<pid>`.
pub fn normalize(text: &str) -> String {
    let trimmed = text.trim();
    let pid_replaced = replace_pids(trimmed);
    collapse_whitespace(&pid_replaced)
}

fn replace_pids(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(end) = find_pid_end(&bytes[i..]) {
                out.push_str("<pid>");
                i += end;
                continue;
            }
        }
        let ch = s[i..].chars().next().expect("valid utf-8");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn find_pid_end(slice: &[u8]) -> Option<usize> {
    if slice.first()? != &b'<' {
        return None;
    }
    let mut idx = 1;
    let mut dots = 0;
    let mut digits_in_segment = 0;
    while idx < slice.len() {
        let c = slice[idx];
        if c.is_ascii_digit() {
            digits_in_segment += 1;
            idx += 1;
            continue;
        }
        if c == b'.' {
            if digits_in_segment == 0 {
                return None;
            }
            dots += 1;
            digits_in_segment = 0;
            idx += 1;
            continue;
        }
        if c == b'>' && dots == 2 && digits_in_segment > 0 {
            return Some(idx + 1);
        }
        return None;
    }
    None
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
}

// ── Driver ──────────────────────────────────────────────────────────────────

/// The outcome of running a single scenario.
#[derive(Debug)]
pub struct Outcome {
    /// The scenario that was run.
    pub scenario: Scenario,
    /// `Ok(())` on success, `Err(message)` with a clear diff on failure.
    pub result: Result<(), String>,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.result {
            Ok(()) => write!(f, "  PASS  {}", self.scenario.name),
            Err(msg) => write!(
                f,
                "  FAIL  {}\n{}",
                self.scenario.name,
                indent(msg, "        ")
            ),
        }
    }
}

/// Run a single scenario: stage, build, execute, and assert.
pub fn run(tc: &Toolchain, scenario: &Scenario) -> Outcome {
    let result = run_inner(tc, scenario);
    Outcome {
        scenario: scenario.clone(),
        result,
    }
}

fn run_inner(tc: &Toolchain, scenario: &Scenario) -> Result<(), String> {
    // `Scenario` is public with public fields, so a caller could hand us a
    // vector the parser would never produce. Re-check the step invariants here
    // rather than indexing `steps[0]` blind (panic) or iterating zero steps on a
    // session surface (vacuous green) — the worst failure mode for this gate.
    check_step_invariants(scenario)?;

    let staged = crate::stage_project(&scenario.dir_name);
    let path = staged.path();

    match scenario.surface {
        Surface::Bunit => run_bunit(tc, path, &scenario.name),
        Surface::Run => run_entrypoint(tc, path, scenario, &scenario.steps[0]),
        Surface::Cli => run_cli(tc, path, scenario, &scenario.steps[0]),
        Surface::Lsp => run_lsp(tc, path, scenario),
        Surface::Mcp => run_mcp(tc, path, scenario),
    }
}

/// Guard the step-count invariants `run_inner` relies on: every scenario has at
/// least one step, and a non-session surface (`bunit`/`run`/`cli`) has exactly
/// one. Discovery already enforces these, but the public `Scenario` API can't,
/// so they are re-checked before dispatch.
fn check_step_invariants(scenario: &Scenario) -> Result<(), String> {
    if scenario.steps.is_empty() {
        return Err(format!("scenario `{}` has no steps", scenario.name));
    }
    if !scenario.surface.is_session_backed() && scenario.steps.len() != 1 {
        return Err(format!(
            "scenario `{}`: `{}` surface expects exactly one step, got {}",
            scenario.name,
            surface_name(scenario.surface),
            scenario.steps.len()
        ));
    }
    Ok(())
}

/// Label a step for failure messages: bare `(detail)` for a single-step
/// scenario, `step k/n (detail)` for a sequence.
fn step_label(scenario: &Scenario, idx: usize, detail: &str) -> String {
    let n = scenario.steps.len();
    if n > 1 {
        format!("step {}/{n} ({detail})", idx + 1)
    } else {
        format!("({detail})")
    }
}

/// Drive an `mcp` scenario: start `beamtalk-mcp --start` (which boots a live
/// workspace) once, run every step against that one session, then assert each
/// step's response substring.
fn run_mcp(tc: &Toolchain, project: &Path, scenario: &Scenario) -> Result<(), String> {
    let bin_dir = tc
        .bin
        .parent()
        .ok_or("could not locate bundle bin/ dir")?
        .to_path_buf();
    let mcp_bin = bin_dir.join(if cfg!(windows) {
        "beamtalk-mcp.exe"
    } else {
        "beamtalk-mcp"
    });
    if !mcp_bin.exists() {
        return Err(format!(
            "mcp binary not found at {} (bundle layout may have changed)",
            mcp_bin.display()
        ));
    }

    let result = (|| -> Result<(), String> {
        let mut client = crate::mcp::McpClient::start(&mcp_bin, project, &bin_dir)?;
        client.initialize()?;

        for (i, step) in scenario.steps.iter().enumerate() {
            let tool = step.tool.as_deref().unwrap();
            let label = step_label(scenario, i, &format!("mcp `{tool}`"));

            // `code = "..."` is a shortcut for the evaluate shape; otherwise use
            // the raw `arguments` JSON, defaulting to `{}`.
            let arguments = if let Some(code) = step.code.as_deref() {
                format!(r#"{{"code":"{}"}}"#, crate::lsp::json_escape(code))
            } else {
                step.arguments.clone().unwrap_or_else(|| "{}".to_string())
            };

            let response = client.call_tool(tool, &arguments)?;

            // A JSON-RPC error, or a tool result flagged `isError`, is a failure.
            if response.contains("\"error\"") && !response.contains("\"isError\":false") {
                return Err(format!(
                    "scenario `{}` {label}: server returned an error:\n{response}",
                    scenario.name
                ));
            }
            if response.contains("\"isError\":true") || response.contains("\"isError\": true") {
                return Err(format!(
                    "scenario `{}` {label}: tool reported isError:\n{response}",
                    scenario.name
                ));
            }
            if let Some(needle) = step.response_contains.as_deref() {
                if !response.contains(needle) {
                    return Err(format!(
                        "scenario `{}` {label}: response missing expected substring {needle:?}\n--- response ---\n{response}",
                        scenario.name
                    ));
                }
            }
        }
        Ok(())
    })();

    // Best-effort: stop the workspace `--start` spawned so it doesn't linger.
    // Fire-and-forget — never let cleanup block or fail the scenario.
    let mut path = std::ffi::OsString::from(bin_dir.as_os_str());
    if let Some(existing) = std::env::var_os("PATH") {
        path.push(if cfg!(windows) { ";" } else { ":" });
        path.push(existing);
    }
    let _ = tc
        .command()
        .args(["workspace", "stop"])
        .current_dir(project)
        .env("PATH", path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    result
}

/// Drive an `lsp` scenario: start `beamtalk-lsp` once, then for each step open
/// its source file, send the declared request, and assert the response.
fn run_lsp(tc: &Toolchain, project: &Path, scenario: &Scenario) -> Result<(), String> {
    // The language server ships next to `beamtalk` in the bundle's `bin/`.
    let lsp_bin = tc.bin.with_file_name(if cfg!(windows) {
        "beamtalk-lsp.exe"
    } else {
        "beamtalk-lsp"
    });
    if !lsp_bin.exists() {
        return Err(format!(
            "lsp binary not found at {} (bundle layout may have changed)",
            lsp_bin.display()
        ));
    }

    let mut client = crate::lsp::LspClient::start(&lsp_bin)?;
    let root_uri = path_to_uri(project);
    client.initialize(&root_uri)?;

    // A document is opened at most once per session: `did_open` always sends
    // `version: 1`, so re-opening an already-open URI across steps is a protocol
    // violation. Steps that revisit the same source just reuse the open doc.
    let mut opened = std::collections::HashSet::new();

    for (i, step) in scenario.steps.iter().enumerate() {
        let method = step.lsp_method.as_deref().unwrap();
        // Optional in a sequence: a side-effect step need only not error.
        let needle = step.response_contains.as_deref();
        let label = step_label(scenario, i, &format!("`{method}`"));

        let src_rel = step.source.as_deref().unwrap();
        let src_path = project.join(src_rel);
        let text = std::fs::read_to_string(&src_path)
            .map_err(|err| format!("read source {}: {err}", src_path.display()))?;
        let abs = src_path.canonicalize().unwrap_or_else(|_| src_path.clone());
        let uri = path_to_uri(&abs);
        if opened.insert(uri.clone()) {
            client.did_open(&uri, &text)?;
        }

        // Build params: every request targets the open document; position-based
        // ones add a cursor; formatting needs FormattingOptions.
        let td = format!(r#"{{"uri":"{}"}}"#, crate::lsp::json_escape(&uri));
        let params = if lsp_needs_position(method) {
            format!(
                r#"{{"textDocument":{td},"position":{{"line":{},"character":{}}}}}"#,
                step.line.unwrap(),
                step.character.unwrap()
            )
        } else if method == "textDocument/formatting" {
            format!(r#"{{"textDocument":{td},"options":{{"tabSize":2,"insertSpaces":true}}}}"#)
        } else {
            format!(r#"{{"textDocument":{td}}}"#)
        };

        let response = client.request(method, &params)?;

        if response.contains("\"error\"") {
            return Err(format!(
                "scenario `{}` {label}: server returned an error:\n{response}",
                scenario.name
            ));
        }
        if let Some(needle) = needle {
            if !response.contains(needle) {
                return Err(format!(
                    "scenario `{}` {label}: response missing expected substring {needle:?}\n--- response ---\n{response}",
                    scenario.name
                ));
            }
        }
    }
    Ok(())
}

/// Build a `file://` URI from an absolute path (Unix; Windows adds the leading
/// slash and forward slashes the separators).
fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// Drive a `cli` scenario: run `beamtalk <args>` in the staged project dir and
/// assert exit code (defaulting to `0`) plus optional stdout/stderr substrings.
fn run_cli(tc: &Toolchain, project: &Path, scenario: &Scenario, step: &Step) -> Result<(), String> {
    let raw = step.args.as_deref().unwrap();
    let args: Vec<&str> = raw.split_whitespace().collect();
    if args.is_empty() {
        return Err(format!(
            "cli scenario `{}` has whitespace-only `args` (original: {raw:?})",
            scenario.name
        ));
    }

    // Optional setup command (e.g. `new <pkg>` to scaffold a project the
    // asserted command then runs against). Must exit 0.
    if let Some(raw_setup) = step.setup.as_deref() {
        let setup_args: Vec<&str> = raw_setup.split_whitespace().collect();
        if setup_args.is_empty() {
            return Err(format!(
                "cli scenario `{}` has whitespace-only `setup` (original: {raw_setup:?})",
                scenario.name
            ));
        }
        let setup_out = tc
            .command()
            .args(&setup_args)
            .current_dir(project)
            .output()
            .map_err(|e| format!("spawn setup `beamtalk {raw_setup}`: {e}"))?;
        if !setup_out.status.success() {
            return Err(format!(
                "scenario `{}` setup `beamtalk {raw_setup}` failed:\n{}",
                scenario.name,
                combined_output(&setup_out)
            ));
        }
    }

    // The asserted command may run in a subdirectory of the staged project
    // (e.g. the package `setup` just scaffolded).
    let work_dir = match step.cwd.as_deref() {
        Some(sub) => project.join(sub),
        None => project.to_path_buf(),
    };

    let out = tc
        .command()
        .args(&args)
        .current_dir(&work_dir)
        .output()
        .map_err(|e| format!("spawn `beamtalk {raw}`: {e}"))?;

    let mut errors = Vec::new();

    // Assert exit code — for cli, an absent `exit_code` means "expect success".
    let expected_code = step.exit_code.unwrap_or(0);
    let actual_code = out.status.code().unwrap_or(-1);
    if actual_code != expected_code {
        errors.push(format!(
            "exit code: expected {expected_code}, got {actual_code}"
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    if let Some(ref needle) = step.stdout_contains {
        if !stdout.contains(needle.as_str()) {
            errors.push(format!("stdout missing expected substring: {needle:?}"));
        }
    }
    if let Some(ref needle) = step.stderr_contains {
        if !stderr.contains(needle.as_str()) {
            errors.push(format!("stderr missing expected substring: {needle:?}"));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "scenario `{}` (`beamtalk {raw}`):\n{}\n--- full output ---\n{}",
            scenario.name,
            errors.join("\n"),
            combined_output(&out)
        ))
    }
}

fn run_bunit(tc: &Toolchain, project: &Path, name: &str) -> Result<(), String> {
    let out = tc
        .command()
        .arg("test")
        .current_dir(project)
        .output()
        .map_err(|e| format!("spawn `beamtalk test`: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "`beamtalk test` exited with {}\n{}",
            out.status,
            combined_output(&out)
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    if !stdout.contains("0 failed") || !stdout.contains("passed") {
        return Err(format!(
            "scenario `{name}`: expected a passing BUnit run (\"0 failed\" + \"passed\"), got:\n{}",
            combined_output(&out)
        ));
    }

    Ok(())
}

fn run_entrypoint(
    tc: &Toolchain,
    project: &Path,
    scenario: &Scenario,
    step: &Step,
) -> Result<(), String> {
    let entrypoint = step.entrypoint.as_deref().unwrap();
    let parts: Vec<&str> = entrypoint.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!(
            "entrypoint `{entrypoint}` must be `Class selector` (at least two tokens)"
        ));
    }

    // `beamtalk run` needs a built project first.
    let build_out = tc
        .command()
        .arg("build")
        .current_dir(project)
        .output()
        .map_err(|e| format!("spawn `beamtalk build`: {e}"))?;

    if !build_out.status.success() {
        return Err(format!(
            "`beamtalk build` failed (scenario `{}`):\n{}",
            scenario.name,
            combined_output(&build_out)
        ));
    }

    let out = tc
        .command()
        .arg("run")
        .args(&parts)
        .current_dir(project)
        .output()
        .map_err(|e| format!("spawn `beamtalk run {entrypoint}`: {e}"))?;

    let mut errors = Vec::new();

    // Assert exit code.
    if let Some(expected_code) = step.exit_code {
        let actual_code = out.status.code().unwrap_or(-1);
        if actual_code != expected_code {
            errors.push(format!(
                "exit code: expected {expected_code}, got {actual_code}"
            ));
        }
    }

    // Assert stdout.
    if let Some(ref expected_stdout) = step.stdout {
        let actual = normalize(&String::from_utf8_lossy(&out.stdout));
        let expected = normalize(expected_stdout);
        if actual != expected {
            errors.push(format!(
                "stdout mismatch:\n  expected: {expected}\n  actual:   {actual}"
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "scenario `{}` (`beamtalk run {entrypoint}`):\n{}\n--- full output ---\n{}",
            scenario.name,
            errors.join("\n"),
            combined_output(&out)
        ))
    }
}

/// Run all discovered scenarios and return their outcomes.
pub fn run_all(tc: &Toolchain, scenarios: &[Scenario]) -> Vec<Outcome> {
    scenarios.iter().map(|s| run(tc, s)).collect()
}

/// Format a combined stdout + stderr dump for failure messages.
fn combined_output(out: &std::process::Output) -> String {
    format!(
        "--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn indent(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|l| format!("{prefix}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a flat (single-scenario) `expect.toml`, returning its one scenario.
    fn parse_one(text: &str) -> Result<Scenario, String> {
        let mut v = parse_expect_text(text, "d", Path::new("/x"))?;
        assert_eq!(v.len(), 1, "expected exactly one scenario");
        Ok(v.pop().unwrap())
    }

    #[test]
    fn parse_bunit_expect() {
        let s = parse_one("surface = \"bunit\"\n").unwrap();
        assert_eq!(s.surface, Surface::Bunit);
        assert_eq!(s.name, "d");
        assert_eq!(s.steps.len(), 1);
    }

    #[test]
    fn parse_run_expect() {
        let s = parse_one(
            "surface = \"run\"\nentrypoint = \"Greeter greet\"\nstdout = \"Hello!\"\nexit_code = 0\n",
        )
        .unwrap();
        assert_eq!(s.surface, Surface::Run);
        let step = &s.steps[0];
        assert_eq!(step.entrypoint.as_deref(), Some("Greeter greet"));
        assert_eq!(step.stdout.as_deref(), Some("Hello!"));
        assert_eq!(step.exit_code, Some(0));
    }

    #[test]
    fn rejects_run_without_entrypoint() {
        let err = parse_one("surface = \"run\"\nstdout = \"hi\"\n").unwrap_err();
        assert!(err.contains("entrypoint"));
    }

    #[test]
    fn rejects_run_without_assertion() {
        let err = parse_one("surface = \"run\"\nentrypoint = \"Foo bar\"\n").unwrap_err();
        assert!(err.contains("stdout"));
    }

    #[test]
    fn parse_cli_expect() {
        let s = parse_one(
            "surface = \"cli\"\nargs = \"lint --format json\"\nexit_code = 1\nstdout_contains = \"severity\"\n",
        )
        .unwrap();
        assert_eq!(s.surface, Surface::Cli);
        let step = &s.steps[0];
        assert_eq!(step.args.as_deref(), Some("lint --format json"));
        assert_eq!(step.exit_code, Some(1));
        assert_eq!(step.stdout_contains.as_deref(), Some("severity"));
    }

    #[test]
    fn parse_cli_expect_with_setup_and_cwd() {
        let s = parse_one(
            "surface = \"cli\"\nsetup = \"new pkg\"\ncwd = \"pkg\"\nargs = \"fmt-check\"\nexit_code = 0\n",
        )
        .unwrap();
        let step = &s.steps[0];
        assert_eq!(step.setup.as_deref(), Some("new pkg"));
        assert_eq!(step.cwd.as_deref(), Some("pkg"));
        assert_eq!(step.args.as_deref(), Some("fmt-check"));
    }

    #[test]
    fn rejects_cli_without_args() {
        let err = parse_one("surface = \"cli\"\nexit_code = 0\n").unwrap_err();
        assert!(err.contains("args"));
    }

    #[test]
    fn cli_exit_code_absent_is_allowed() {
        let s = parse_one("surface = \"cli\"\nargs = \"--help\"\n").unwrap();
        assert_eq!(s.steps[0].exit_code, None);
    }

    #[test]
    fn parse_lsp_expect() {
        let s = parse_one(
            "surface = \"lsp\"\nmethod = \"textDocument/hover\"\nsource = \"src/Foo.bt\"\nline = 4\ncharacter = 18\nresponse_contains = \"Extends:\"\n",
        )
        .unwrap();
        assert_eq!(s.surface, Surface::Lsp);
        let step = &s.steps[0];
        assert_eq!(step.lsp_method.as_deref(), Some("textDocument/hover"));
        assert_eq!(step.source.as_deref(), Some("src/Foo.bt"));
        assert_eq!(step.line, Some(4));
        assert_eq!(step.character, Some(18));
    }

    #[test]
    fn lsp_document_symbol_needs_no_position() {
        let s = parse_one(
            "surface = \"lsp\"\nmethod = \"textDocument/documentSymbol\"\nsource = \"src/Foo.bt\"\nresponse_contains = \"increment:\"\n",
        )
        .unwrap();
        assert_eq!(s.steps[0].line, None);
        assert_eq!(s.steps[0].character, None);
    }

    #[test]
    fn rejects_lsp_without_required_keys() {
        let err = parse_one(
            "surface = \"lsp\"\nmethod = \"textDocument/hover\"\nresponse_contains = \"x\"\nline = 0\ncharacter = 0\n",
        )
        .unwrap_err();
        assert!(err.contains("source"));

        let err = parse_one(
            "surface = \"lsp\"\nmethod = \"textDocument/hover\"\nsource = \"src/Foo.bt\"\nresponse_contains = \"x\"\n",
        )
        .unwrap_err();
        assert!(err.contains("line") && err.contains("character"));
    }

    #[test]
    fn parse_mcp_expect() {
        let s = parse_one(
            "surface = \"mcp\"\ntool = \"evaluate\"\ncode = \"1 + 1\"\nresponse_contains = \"2\"\n",
        )
        .unwrap();
        assert_eq!(s.surface, Surface::Mcp);
        let step = &s.steps[0];
        assert_eq!(step.tool.as_deref(), Some("evaluate"));
        assert_eq!(step.code.as_deref(), Some("1 + 1"));
        assert_eq!(step.response_contains.as_deref(), Some("2"));
    }

    #[test]
    fn rejects_mcp_without_tool() {
        let err = parse_one("surface = \"mcp\"\nresponse_contains = \"2\"\n").unwrap_err();
        assert!(err.contains("tool"));
    }

    #[test]
    fn rejects_mcp_code_and_arguments() {
        let err = parse_one(
            "surface = \"mcp\"\ntool = \"evaluate\"\ncode = \"1\"\narguments = \"{}\"\nresponse_contains = \"x\"\n",
        )
        .unwrap_err();
        assert!(err.contains("code") && err.contains("arguments"));
    }

    // ── BT-2480: fan-out + sequence forms ────────────────────────────────────

    #[test]
    fn fan_out_yields_one_scenario_per_entry() {
        let text = r#"
[[scenario]]
name = "hover"
surface = "lsp"
method = "textDocument/hover"
source = "src/Foo.bt"
line = 4
character = 18
response_contains = "Extends:"

[[scenario]]
name = "symbols"
surface = "lsp"
method = "textDocument/documentSymbol"
source = "src/Foo.bt"
response_contains = "increment:"
"#;
        let scns = parse_expect_text(text, "lsp", Path::new("/x")).unwrap();
        assert_eq!(scns.len(), 2);
        assert_eq!(scns[0].name, "lsp/hover");
        assert_eq!(scns[1].name, "lsp/symbols");
        // Both stage from the same fixture dir.
        assert_eq!(scns[0].dir_name, "lsp");
        assert_eq!(scns[1].dir_name, "lsp");
    }

    #[test]
    fn mcp_step_sequence_parses_multiple_steps() {
        let text = r#"
[[scenario]]
name = "create-and-list"
surface = "mcp"
[[scenario.step]]
tool = "evaluate"
code = "Counter spawn"
[[scenario.step]]
tool = "workspace_actors"
response_contains = "Counter"
"#;
        let scns = parse_expect_text(text, "actors", Path::new("/x")).unwrap();
        assert_eq!(scns.len(), 1);
        assert_eq!(scns[0].name, "actors/create-and-list");
        assert_eq!(scns[0].steps.len(), 2);
        // First step is a side effect with no assertion — allowed in a sequence.
        assert_eq!(scns[0].steps[0].response_contains, None);
        assert_eq!(
            scns[0].steps[1].response_contains.as_deref(),
            Some("Counter")
        );
    }

    #[test]
    fn flat_step_sequence_parses() {
        let text = r#"
surface = "mcp"
[[step]]
tool = "evaluate"
code = "Counter spawn"
[[step]]
tool = "evaluate"
code = "1 + 1"
response_contains = "2"
"#;
        let s = parse_one(text).unwrap();
        assert_eq!(s.steps.len(), 2);
    }

    #[test]
    fn rejects_steps_on_non_session_surface() {
        let text = r#"
surface = "cli"
[[step]]
args = "lint"
"#;
        let err = parse_expect_text(text, "d", Path::new("/x")).unwrap_err();
        assert!(err.contains("session-backed"));
    }

    #[test]
    fn rejects_mixing_inline_action_with_steps() {
        let text = r#"
surface = "mcp"
tool = "evaluate"
code = "1"
response_contains = "1"
[[step]]
tool = "evaluate"
code = "2"
response_contains = "2"
"#;
        let err = parse_expect_text(text, "d", Path::new("/x")).unwrap_err();
        assert!(err.contains("not both"));
    }

    #[test]
    fn rejects_field_from_another_surface() {
        // A `cli` scenario carrying a `run`/`mcp` field would silently ignore
        // it at runtime — reject at parse instead.
        let err = parse_one("surface = \"cli\"\nargs = \"--help\"\nstdout = \"ok\"\n").unwrap_err();
        assert!(err.contains("does not use") && err.contains("stdout"));

        let err = parse_one("surface = \"bunit\"\ntool = \"evaluate\"\n").unwrap_err();
        assert!(err.contains("tool"));
    }

    #[test]
    fn lsp_sequence_allows_side_effect_step_without_assertion() {
        let text = r#"
[[scenario]]
name = "seq"
surface = "lsp"
[[scenario.step]]
method = "textDocument/documentSymbol"
source = "src/Foo.bt"
[[scenario.step]]
method = "textDocument/documentSymbol"
source = "src/Foo.bt"
response_contains = "increment:"
"#;
        let scns = parse_expect_text(text, "lsp", Path::new("/x")).unwrap();
        assert_eq!(scns[0].steps.len(), 2);
        assert_eq!(scns[0].steps[0].response_contains, None);
    }

    #[test]
    fn rejects_flat_fields_mixed_with_scenario_array() {
        // A file with both top-level flat fields and a `[[scenario]]` array
        // must error, not silently drop the flat scenario.
        let text = r#"
surface = "mcp"
tool = "evaluate"
code = "1"
response_contains = "1"

[[scenario]]
name = "other"
surface = "mcp"
tool = "evaluate"
code = "2"
response_contains = "2"
"#;
        assert!(parse_expect_text(text, "d", Path::new("/x")).is_err());
    }

    // ── BT-2481: typo'd keys must be rejected, not silently dropped ──────────

    #[test]
    fn rejects_typod_key_in_flat_form() {
        // `stdout_contians` (typo) would flatten to `None` and the substring
        // check would never run — the scenario would go green asserting nothing.
        let err =
            parse_one("surface = \"cli\"\nargs = \"lint\"\nstdout_contians = \"never checked\"\n")
                .unwrap_err();
        assert!(err.contains("unknown key") && err.contains("stdout_contians"));
    }

    #[test]
    fn rejects_typod_key_in_scenario_array() {
        let text = r#"
[[scenario]]
name = "hover"
surface = "lsp"
method = "textDocument/hover"
source = "src/Foo.bt"
line = 4
character = 18
response_contians = "Extends:"
"#;
        let err = parse_expect_text(text, "lsp", Path::new("/x")).unwrap_err();
        assert!(err.contains("unknown key") && err.contains("response_contians"));
    }

    #[test]
    fn rejects_typod_key_in_step() {
        let text = r#"
surface = "mcp"
[[step]]
tool = "evaluate"
code = "Counter spawn"
[[step]]
tool = "workspace_actors"
response_contians = "Counter"
"#;
        let err = parse_expect_text(text, "d", Path::new("/x")).unwrap_err();
        assert!(err.contains("unknown step key") && err.contains("response_contians"));
    }

    #[test]
    fn rejects_unknown_top_level_key_alongside_scenario_array() {
        let text = r#"
bogus = "x"
[[scenario]]
name = "a"
surface = "cli"
args = "lint"
"#;
        let err = parse_expect_text(text, "d", Path::new("/x")).unwrap_err();
        assert!(err.contains("unknown top-level key") && err.contains("bogus"));
    }

    fn scenario_with_steps(surface: Surface, steps: Vec<Step>) -> Scenario {
        Scenario {
            name: "s".to_string(),
            dir_name: "s".to_string(),
            project_dir: PathBuf::from("/x"),
            surface,
            steps,
        }
    }

    #[test]
    fn rejects_scenario_with_no_steps() {
        let s = scenario_with_steps(Surface::Cli, vec![]);
        let err = check_step_invariants(&s).unwrap_err();
        assert!(err.contains("no steps"));
    }

    #[test]
    fn rejects_multi_step_on_non_session_surface() {
        let s = scenario_with_steps(Surface::Cli, vec![Step::default(), Step::default()]);
        let err = check_step_invariants(&s).unwrap_err();
        assert!(err.contains("exactly one step"));
    }

    #[test]
    fn accepts_single_step_and_session_sequences() {
        assert!(
            check_step_invariants(&scenario_with_steps(Surface::Cli, vec![Step::default()]))
                .is_ok()
        );
        assert!(check_step_invariants(&scenario_with_steps(
            Surface::Mcp,
            vec![Step::default(), Step::default()]
        ))
        .is_ok());
    }

    #[test]
    fn parse_skips_comments() {
        let s = parse_one("# a comment\nsurface = \"cli\"\nargs = \"lint\"  # trailing\n").unwrap();
        assert_eq!(s.steps[0].args.as_deref(), Some("lint"));
    }

    #[test]
    fn normalize_trims_and_collapses() {
        assert_eq!(normalize("  hello   world  \n"), "hello world");
    }

    #[test]
    fn normalize_replaces_pids() {
        assert_eq!(normalize("<0.123.0> ready"), "<pid> ready");
        assert_eq!(normalize("<0.1.0> and <0.2.0>"), "<pid> and <pid>");
    }

    #[test]
    fn normalize_keeps_non_pids() {
        assert_eq!(normalize("<not a pid>"), "<not a pid>");
    }

    #[test]
    fn discover_finds_scenarios() {
        let root = std::env::temp_dir().join("bt-uat-test-discover");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        std::fs::create_dir_all(root.join("beta")).unwrap();
        std::fs::create_dir_all(root.join("no-expect")).unwrap();

        std::fs::write(
            root.join("alpha").join("expect.toml"),
            "surface = \"bunit\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("beta").join("expect.toml"),
            "surface = \"run\"\nentrypoint = \"B run\"\nstdout = \"ok\"\n",
        )
        .unwrap();
        // no-expect has no expect.toml — should be skipped.

        let scenarios = discover(&root).unwrap();
        assert_eq!(scenarios.len(), 2);
        assert_eq!(scenarios[0].name, "alpha");
        assert_eq!(scenarios[1].name, "beta");

        let _ = std::fs::remove_dir_all(&root);
    }
}
