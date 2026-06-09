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

#[test]
#[ignore = "requires network + Erlang/OTP; run via `just uat`"]
fn all_scenarios_pass() {
    let tc = shared();
    let scenarios = scenario::discover(&projects_dir()).expect("failed to discover scenarios");

    assert!(
        !scenarios.is_empty(),
        "no scenarios found in {}",
        projects_dir().display()
    );

    println!("\n--- UAT scenarios ({}) ---", scenarios.len());
    for s in &scenarios {
        // Show the most relevant detail per surface: entrypoint for `run`, args
        // for `cli`, the request method for `lsp`, nothing extra for `bunit`.
        let detail = s
            .expect
            .entrypoint
            .as_deref()
            .or(s.expect.args.as_deref())
            .or(s.expect.lsp_method.as_deref())
            .unwrap_or("-");
        println!(
            "  {:24} surface={:5} {}",
            s.name,
            format!("{:?}", s.expect.surface),
            detail
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
