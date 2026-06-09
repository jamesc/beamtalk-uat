// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

//! Scenario discovery, parsing, and assertion driver (BT-2450).
//!
//! A **scenario** is a directory under `projects/<name>/` containing:
//!
//! * A valid `beamtalk.toml` package (sources in `src/`, optional tests in
//!   `test/`).
//! * An `expect.toml` file declaring the assertion surface, expected output,
//!   and (for `run` scenarios) the entrypoint.
//!
//! ## `expect.toml` format
//!
//! ```toml
//! # -- BUnit scenario (the default, preferred for deterministic pass/fail) --
//! surface = "bunit"
//! # No other fields needed; the driver runs `beamtalk test` and asserts all
//! # tests pass (exit 0, "0 failed" in stdout).
//!
//! # -- Run scenario (script mode: `beamtalk run Class selector`) --
//! surface = "run"
//! entrypoint = "Greeter greet"
//! # At least one of `stdout` or `exit_code` must be present.
//! stdout = "Hello from smoke!"
//! exit_code = 0
//!
//! # -- CLI scenario (drive a `beamtalk` subcommand directly) --
//! surface = "cli"
//! args = "lint --format json"   # appended to `beamtalk`, whitespace-split
//! exit_code = 1                  # optional; defaults to 0 (success)
//! stdout_contains = "severity"  # optional substring assertion
//! stderr_contains = "error"     # optional substring assertion
//! ```
//!
//! A `cli` scenario runs `beamtalk <args>` in the staged project directory (a
//! throwaway temp copy, so commands that mutate the tree — `fmt`, `new` — are
//! isolated). It is the surface for the build/tooling commands that have no REPL
//! op: `new`, `fmt`, `fmt-check`, `lint`, `type-coverage`, `build`, `--help`, …
//! Assertions use **substring** matching (not exact, like `run`) because CLI
//! output embeds absolute paths and version strings that vary per environment.
//!
//! ## Output normalization
//!
//! Before comparing `stdout`, both expected and actual values are normalized:
//! leading/trailing whitespace trimmed, internal runs of whitespace collapsed
//! to a single space, and Erlang PIDs (`<0.123.0>`) replaced with `<pid>`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::Toolchain;

// ── Expectation types ───────────────────────────────────────────────────────

/// The assertion surface a scenario exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

/// A parsed `expect.toml` for one scenario.
#[derive(Debug, Clone)]
pub struct Expectation {
    /// Which surface to exercise.
    pub surface: Surface,
    /// `Class selector` entrypoint (required for `run`, ignored elsewhere).
    pub entrypoint: Option<String>,
    /// Arguments appended to `beamtalk` for a `cli` scenario, whitespace-split
    /// (e.g. `"lint --format json"`). Required for `cli`, ignored elsewhere.
    pub args: Option<String>,
    /// Expected (normalized) stdout, compared *exactly* after normalization
    /// (`run` surface).
    pub stdout: Option<String>,
    /// Substring expected to appear in stdout (`cli` surface).
    pub stdout_contains: Option<String>,
    /// Substring expected to appear in stderr (`cli` surface).
    pub stderr_contains: Option<String>,
    /// Expected process exit code. For `cli` it defaults to `0` (success); for
    /// `run` it is only asserted when present.
    pub exit_code: Option<i32>,
    /// LSP request method to send (`lsp` surface), e.g.
    /// `"textDocument/documentSymbol"`.
    pub lsp_method: Option<String>,
    /// Project-relative path of the source file to open (`lsp` surface).
    pub source: Option<String>,
    /// 0-based cursor line for position-based LSP requests (`lsp` surface).
    pub line: Option<u32>,
    /// 0-based cursor character for position-based LSP requests (`lsp` surface).
    pub character: Option<u32>,
    /// Substring expected in the LSP response (`lsp` surface).
    pub response_contains: Option<String>,
}

/// A discovered scenario ready to be driven.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// Human-readable name (the directory name under `projects/`).
    pub name: String,
    /// Path to the project directory (inside the repo, not yet staged).
    pub project_dir: PathBuf,
    /// Parsed expectation.
    pub expect: Expectation,
}

// ── Discovery ───────────────────────────────────────────────────────────────

/// Discover all scenarios under `projects/` that have an `expect.toml`.
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
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let expect =
            parse_expect_toml(&expect_path).map_err(|e| format!("scenario `{name}`: {e}"))?;
        scenarios.push(Scenario {
            name,
            project_dir: dir,
            expect,
        });
    }
    scenarios.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(scenarios)
}

// ── TOML parser (hand-rolled, no dependencies) ─────────────────────────────

/// Parse an `expect.toml` file into an [`Expectation`].
///
/// We deliberately avoid pulling in a TOML crate to stay dependency-free. The
/// format is intentionally minimal (flat key = value, no nested tables), so a
/// line-oriented parser is sufficient.
fn parse_expect_toml(path: &Path) -> Result<Expectation, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let kv = parse_flat_toml(&text)?;

    let surface = match kv.get("surface").map(|s| s.as_str()) {
        Some("bunit") => Surface::Bunit,
        Some("run") => Surface::Run,
        Some("cli") => Surface::Cli,
        Some("lsp") => Surface::Lsp,
        Some(other) => {
            return Err(format!(
                "unknown surface `{other}` (expected `bunit`, `run`, `cli`, or `lsp`)"
            ))
        }
        None => return Err("missing required key `surface`".to_string()),
    };

    let entrypoint = kv.get("entrypoint").cloned();
    let args = kv.get("args").cloned();
    let stdout = kv.get("stdout").cloned();
    let stdout_contains = kv.get("stdout_contains").cloned();
    let stderr_contains = kv.get("stderr_contains").cloned();
    let exit_code = kv
        .get("exit_code")
        .map(|v| {
            v.parse::<i32>()
                .map_err(|e| format!("invalid exit_code `{v}`: {e}"))
        })
        .transpose()?;
    let lsp_method = kv.get("method").cloned();
    let source = kv.get("source").cloned();
    let parse_u32 = |key: &str| -> Result<Option<u32>, String> {
        kv.get(key)
            .map(|v| {
                v.parse::<u32>()
                    .map_err(|e| format!("invalid {key} `{v}`: {e}"))
            })
            .transpose()
    };
    let line = parse_u32("line")?;
    let character = parse_u32("character")?;
    let response_contains = kv.get("response_contains").cloned();

    // Validate: run scenarios need an entrypoint.
    if surface == Surface::Run && entrypoint.is_none() {
        return Err("`surface = \"run\"` requires an `entrypoint` key".to_string());
    }
    // Validate: run scenarios need at least one assertion.
    if surface == Surface::Run && stdout.is_none() && exit_code.is_none() {
        return Err(
            "`surface = \"run\"` requires at least one of `stdout` or `exit_code`".to_string(),
        );
    }
    // Validate: cli scenarios need a command to run.
    if surface == Surface::Cli && args.is_none() {
        return Err("`surface = \"cli\"` requires an `args` key".to_string());
    }
    // Validate: lsp scenarios need a method, a source file, and an assertion.
    if surface == Surface::Lsp {
        if lsp_method.is_none() {
            return Err("`surface = \"lsp\"` requires a `method` key".to_string());
        }
        if source.is_none() {
            return Err("`surface = \"lsp\"` requires a `source` key".to_string());
        }
        if response_contains.is_none() {
            return Err("`surface = \"lsp\"` requires a `response_contains` key".to_string());
        }
        // Position-based requests need a cursor.
        if lsp_needs_position(lsp_method.as_deref().unwrap())
            && (line.is_none() || character.is_none())
        {
            return Err(format!(
                "lsp method `{}` requires `line` and `character`",
                lsp_method.as_deref().unwrap()
            ));
        }
    }

    Ok(Expectation {
        surface,
        entrypoint,
        args,
        stdout,
        stdout_contains,
        stderr_contains,
        exit_code,
        lsp_method,
        source,
        line,
        character,
        response_contains,
    })
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

/// Parse a flat TOML file (no tables, arrays, or inline tables) into a
/// `BTreeMap<key, value>`. String values are unquoted; bare values are kept
/// as-is. Comments (`#`) and blank lines are skipped.
fn parse_flat_toml(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = trimmed.split_once('=') else {
            return Err(format!(
                "line {}: expected `key = value`, got `{trimmed}`",
                i + 1
            ));
        };
        let key = key.trim().to_string();
        let value = unquote_toml_value(rest.trim());
        map.insert(key, value);
    }
    Ok(map)
}

/// Strip a trailing inline comment (`# ...`) from a bare (unquoted) value.
///
/// TOML allows `key = value  # comment` but our flat parser doesn't distinguish
/// that from the value itself. We only strip for bare values — quoted strings
/// are handled by `unquote_toml_value` which already stops at the closing `"`.
fn strip_inline_comment(s: &str) -> &str {
    // If the value is quoted, the comment is outside the quotes — don't strip.
    if s.starts_with('"') {
        return s;
    }
    // Find the first `#` and trim trailing whitespace before it.
    match s.find('#') {
        Some(idx) => s[..idx].trim_end(),
        None => s,
    }
}

/// Strip surrounding double quotes from a TOML string value; pass bare values
/// through unchanged.
fn unquote_toml_value(s: &str) -> String {
    let s = strip_inline_comment(s);
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        // Handle basic TOML string escapes.
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('\\') => out.push('\\'),
                    Some('"') => out.push('"'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        s.to_string()
    }
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
    let staged = crate::stage_project(&scenario.name);

    match scenario.expect.surface {
        Surface::Bunit => run_bunit(tc, staged.path(), &scenario.name),
        Surface::Run => run_entrypoint(tc, staged.path(), scenario),
        Surface::Cli => run_cli(tc, staged.path(), scenario),
        Surface::Lsp => run_lsp(tc, staged.path(), scenario),
    }
}

/// Drive an `lsp` scenario: start `beamtalk-lsp`, open the source file, send the
/// declared request, and assert the response contains the expected substring.
fn run_lsp(tc: &Toolchain, project: &Path, scenario: &Scenario) -> Result<(), String> {
    let e = &scenario.expect;
    let method = e.lsp_method.as_deref().unwrap();
    let needle = e.response_contains.as_deref().unwrap();

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

    let src_rel = e.source.as_deref().unwrap();
    let src_path = project.join(src_rel);
    let text = std::fs::read_to_string(&src_path)
        .map_err(|err| format!("read source {}: {err}", src_path.display()))?;
    let abs = src_path.canonicalize().unwrap_or_else(|_| src_path.clone());
    let uri = path_to_uri(&abs);
    let root_uri = path_to_uri(project);

    let mut client = crate::lsp::LspClient::start(&lsp_bin)?;
    client.initialize(&root_uri)?;
    client.did_open(&uri, &text)?;

    // Build params: every request targets the open document; position-based ones
    // add a cursor; formatting needs FormattingOptions.
    let td = format!(r#"{{"uri":"{}"}}"#, crate::lsp::json_escape(&uri));
    let params = if lsp_needs_position(method) {
        format!(
            r#"{{"textDocument":{td},"position":{{"line":{},"character":{}}}}}"#,
            e.line.unwrap(),
            e.character.unwrap()
        )
    } else if method == "textDocument/formatting" {
        format!(r#"{{"textDocument":{td},"options":{{"tabSize":2,"insertSpaces":true}}}}"#)
    } else {
        format!(r#"{{"textDocument":{td}}}"#)
    };

    let response = client.request(method, &params)?;

    if response.contains("\"error\"") {
        return Err(format!(
            "scenario `{}` (`{method}`): server returned an error:\n{response}",
            scenario.name
        ));
    }
    if !response.contains(needle) {
        return Err(format!(
            "scenario `{}` (`{method}`): response missing expected substring {needle:?}\n--- response ---\n{response}",
            scenario.name
        ));
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
fn run_cli(tc: &Toolchain, project: &Path, scenario: &Scenario) -> Result<(), String> {
    let raw = scenario.expect.args.as_deref().unwrap();
    let args: Vec<&str> = raw.split_whitespace().collect();
    if args.is_empty() {
        return Err(format!(
            "cli scenario `{}` has whitespace-only `args` (original: {raw:?})",
            scenario.name
        ));
    }

    let out = tc
        .command()
        .args(&args)
        .current_dir(project)
        .output()
        .map_err(|e| format!("spawn `beamtalk {raw}`: {e}"))?;

    let mut errors = Vec::new();

    // Assert exit code — for cli, an absent `exit_code` means "expect success".
    let expected_code = scenario.expect.exit_code.unwrap_or(0);
    let actual_code = out.status.code().unwrap_or(-1);
    if actual_code != expected_code {
        errors.push(format!(
            "exit code: expected {expected_code}, got {actual_code}"
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    if let Some(ref needle) = scenario.expect.stdout_contains {
        if !stdout.contains(needle.as_str()) {
            errors.push(format!("stdout missing expected substring: {needle:?}"));
        }
    }
    if let Some(ref needle) = scenario.expect.stderr_contains {
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

fn run_entrypoint(tc: &Toolchain, project: &Path, scenario: &Scenario) -> Result<(), String> {
    let entrypoint = scenario.expect.entrypoint.as_deref().unwrap();
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
    if let Some(expected_code) = scenario.expect.exit_code {
        let actual_code = out.status.code().unwrap_or(-1);
        if actual_code != expected_code {
            errors.push(format!(
                "exit code: expected {expected_code}, got {actual_code}"
            ));
        }
    }

    // Assert stdout.
    if let Some(ref expected_stdout) = scenario.expect.stdout {
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

    #[test]
    fn parse_bunit_expect() {
        let toml = "surface = \"bunit\"\n";
        let kv = parse_flat_toml(toml).unwrap();
        assert_eq!(kv["surface"], "bunit");
    }

    #[test]
    fn parse_run_expect() {
        let toml = r#"
surface = "run"
entrypoint = "Greeter greet"
stdout = "Hello!"
exit_code = 0
"#;
        let kv = parse_flat_toml(toml).unwrap();
        assert_eq!(kv["surface"], "run");
        assert_eq!(kv["entrypoint"], "Greeter greet");
        assert_eq!(kv["stdout"], "Hello!");
        assert_eq!(kv["exit_code"], "0");
    }

    #[test]
    fn unquote_handles_escapes() {
        assert_eq!(unquote_toml_value(r#""hello\nworld""#), "hello\nworld");
        assert_eq!(unquote_toml_value(r#""tab\there""#), "tab\there");
        assert_eq!(unquote_toml_value(r#""a\\b""#), "a\\b");
    }

    #[test]
    fn unquote_bare_value() {
        assert_eq!(unquote_toml_value("42"), "42");
    }

    #[test]
    fn inline_comments_stripped_from_bare_values() {
        assert_eq!(unquote_toml_value("0  # optional"), "0");
        assert_eq!(unquote_toml_value("42 # the answer"), "42");
    }

    #[test]
    fn inline_comments_not_stripped_from_quoted_values() {
        // A `#` inside quotes is part of the value; one outside is stripped
        // by `strip_inline_comment` before unquoting.
        assert_eq!(unquote_toml_value(r#""has # inside""#), "has # inside");
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
    fn parse_flat_toml_skips_comments() {
        let toml = "# a comment\nkey = \"val\"\n";
        let kv = parse_flat_toml(toml).unwrap();
        assert_eq!(kv.len(), 1);
        assert_eq!(kv["key"], "val");
    }

    #[test]
    fn parse_expect_toml_rejects_run_without_entrypoint() {
        let dir = std::env::temp_dir().join("bt-uat-test-no-entry");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("expect.toml");
        std::fs::write(&path, "surface = \"run\"\nstdout = \"hi\"\n").unwrap();
        let result = parse_expect_toml(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("entrypoint"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_expect_toml_rejects_run_without_assertion() {
        let dir = std::env::temp_dir().join("bt-uat-test-no-assert");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("expect.toml");
        std::fs::write(&path, "surface = \"run\"\nentrypoint = \"Foo bar\"\n").unwrap();
        let result = parse_expect_toml(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("stdout"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_cli_expect() {
        let dir = std::env::temp_dir().join("bt-uat-test-cli-ok");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("expect.toml");
        std::fs::write(
            &path,
            "surface = \"cli\"\nargs = \"lint --format json\"\nexit_code = 1\nstdout_contains = \"severity\"\n",
        )
        .unwrap();
        let expect = parse_expect_toml(&path).unwrap();
        assert_eq!(expect.surface, Surface::Cli);
        assert_eq!(expect.args.as_deref(), Some("lint --format json"));
        assert_eq!(expect.exit_code, Some(1));
        assert_eq!(expect.stdout_contains.as_deref(), Some("severity"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_expect_toml_rejects_cli_without_args() {
        let dir = std::env::temp_dir().join("bt-uat-test-cli-no-args");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("expect.toml");
        std::fs::write(&path, "surface = \"cli\"\nexit_code = 0\n").unwrap();
        let result = parse_expect_toml(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("args"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_cli_expect_defaults_exit_code_absent() {
        // An absent exit_code is allowed for cli (the driver treats it as 0).
        let dir = std::env::temp_dir().join("bt-uat-test-cli-default");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("expect.toml");
        std::fs::write(&path, "surface = \"cli\"\nargs = \"--help\"\n").unwrap();
        let expect = parse_expect_toml(&path).unwrap();
        assert_eq!(expect.exit_code, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn write_expect(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("expect.toml");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn parse_lsp_expect() {
        let path = write_expect(
            "bt-uat-test-lsp-ok",
            "surface = \"lsp\"\nmethod = \"textDocument/hover\"\nsource = \"src/Foo.bt\"\nline = 4\ncharacter = 18\nresponse_contains = \"Extends:\"\n",
        );
        let e = parse_expect_toml(&path).unwrap();
        assert_eq!(e.surface, Surface::Lsp);
        assert_eq!(e.lsp_method.as_deref(), Some("textDocument/hover"));
        assert_eq!(e.source.as_deref(), Some("src/Foo.bt"));
        assert_eq!(e.line, Some(4));
        assert_eq!(e.character, Some(18));
        assert_eq!(e.response_contains.as_deref(), Some("Extends:"));
    }

    #[test]
    fn parse_lsp_document_symbol_needs_no_position() {
        let path = write_expect(
            "bt-uat-test-lsp-symbols",
            "surface = \"lsp\"\nmethod = \"textDocument/documentSymbol\"\nsource = \"src/Foo.bt\"\nresponse_contains = \"increment:\"\n",
        );
        let e = parse_expect_toml(&path).unwrap();
        assert_eq!(e.line, None);
        assert_eq!(e.character, None);
    }

    #[test]
    fn parse_expect_toml_rejects_lsp_without_required_keys() {
        // Missing source.
        let path = write_expect(
            "bt-uat-test-lsp-no-source",
            "surface = \"lsp\"\nmethod = \"textDocument/hover\"\nresponse_contains = \"x\"\nline = 0\ncharacter = 0\n",
        );
        assert!(parse_expect_toml(&path).unwrap_err().contains("source"));

        // Position method missing the cursor.
        let path = write_expect(
            "bt-uat-test-lsp-no-pos",
            "surface = \"lsp\"\nmethod = \"textDocument/hover\"\nsource = \"src/Foo.bt\"\nresponse_contains = \"x\"\n",
        );
        let err = parse_expect_toml(&path).unwrap_err();
        assert!(err.contains("line") && err.contains("character"));
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
