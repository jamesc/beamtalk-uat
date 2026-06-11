// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

//! General scenario driver test (BT-2450).
//!
//! Discovers every `projects/<name>/` directory that has an `expect.toml`,
//! stages it, builds it with the installed toolchain, runs the declared
//! entrypoint, and asserts the expectation. Adding a new UAT scenario is now:
//!
//! 1. Create `projects/<name>/` with `beamtalk new` (or manually).
//! 2. Add an `expect.toml` declaring surface, entrypoint, and expected output.
//! 3. Done — the driver picks it up automatically.
//!
//! Ignored by default (requires network + Erlang/OTP). Run via `just uat`.

use std::path::PathBuf;

use beamtalk_uat::scenario;
use beamtalk_uat::shared;

fn projects_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("projects")
}

/// Every `expect.toml` in `projects/` parses and discovery is well-formed
/// (unique names, ≥1 step each). Runs offline — no network or Erlang — so a
/// malformed scenario file is caught on any CI leg, not just the e2e ones.
#[test]
fn all_scenarios_discover() {
    let scenarios = scenario::discover(&projects_dir()).expect("failed to discover scenarios");
    assert!(!scenarios.is_empty(), "no scenarios discovered");
    for s in &scenarios {
        assert!(!s.steps.is_empty(), "scenario `{}` has no steps", s.name);
    }
    // The consolidated LSP fixture fans out into one scenario per capability.
    let lsp: Vec<&str> = scenarios
        .iter()
        .map(|s| s.name.as_str())
        .filter(|n| n.starts_with("lsp/"))
        .collect();
    assert!(
        lsp.contains(&"lsp/hover") && lsp.contains(&"lsp/document_symbol"),
        "expected fanned-out lsp scenarios, got {lsp:?}"
    );
}

#[test]
#[ignore = "requires network + Erlang/OTP; run via `just uat`"]
fn all_scenarios_pass() {
    let Some(tc) = shared() else {
        // Rolling pre-release (edge/nightly) not published yet — skip cleanly
        // rather than red the gate (BT-2497).
        eprintln!("skipped: requested rolling release is not available yet");
        return;
    };
    let scenarios = scenario::discover(&projects_dir()).expect("failed to discover scenarios");

    assert!(
        !scenarios.is_empty(),
        "no scenarios found in {}",
        projects_dir().display()
    );

    println!("\n--- UAT scenarios ({}) ---", scenarios.len());
    for s in &scenarios {
        // Show the most relevant detail from the first step: entrypoint for
        // `run`, args for `cli`, the request method for `lsp`, the tool for
        // `mcp`, nothing extra for `bunit`. Stepped scenarios note their length.
        let detail = s
            .steps
            .first()
            .and_then(|step| {
                step.entrypoint
                    .as_deref()
                    .or(step.args.as_deref())
                    .or(step.lsp_method.as_deref())
                    .or(step.tool.as_deref())
            })
            .unwrap_or("-");
        let steps = if s.steps.len() > 1 {
            format!(" [{} steps]", s.steps.len())
        } else {
            String::new()
        };
        println!(
            "  {:24} surface={:5} {}{}",
            s.name,
            format!("{:?}", s.surface),
            detail,
            steps
        );
    }
    println!();

    let outcomes = scenario::run_all(tc, &scenarios);
    let mut failures = Vec::new();

    for outcome in &outcomes {
        println!("{outcome}");
        if outcome.result.is_err() {
            failures.push(outcome);
        }
    }

    if !failures.is_empty() {
        let names: Vec<&str> = failures.iter().map(|o| o.scenario.name.as_str()).collect();
        panic!(
            "\n{} of {} scenarios failed: {}\n",
            failures.len(),
            outcomes.len(),
            names.join(", ")
        );
    }
}
