// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

//! Smoke scenario for the UAT gate (BT-2448): install a released toolchain
//! bundle and prove the basic user flow works end-to-end against a real
//! `beamtalk new` project (`projects/smoke`, sources in `src/`, tests in
//! `test/`) — `beamtalk test` builds the package and runs its BUnit suite.
//!
//! `beamtalk test` is the deterministic assertion surface: BUnit reports
//! pass/fail via exit code, sidestepping script mode's `halt(0)` truncation of
//! stdout. (Value-returning REPL scenarios are driven separately via the
//! interactive REPL.)
//!
//! Ignored by default because it requires network (to fetch the release) and a
//! working Erlang/OTP install. Run via `just uat [version]`, which passes
//! `--ignored`, or set `BEAMTALK_UAT_BIN` to test an already-installed binary.

use beamtalk_uat::{requested_exact_version, shared, stage_project};

fn combined(out: &std::process::Output) -> String {
    format!(
        "--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
#[ignore = "requires network + Erlang/OTP; run via `just uat`"]
fn toolchain_reports_version() {
    let Some(tc) = shared() else {
        eprintln!("skipped: requested rolling release is not available yet");
        return;
    };
    assert!(
        !tc.version.is_empty(),
        "`beamtalk --version` produced no output"
    );
    println!("installed: {} ({})", tc.version, tc.bin.display());

    // When a concrete version was requested, the binary must report it.
    if let Some(v) = requested_exact_version() {
        assert!(
            tc.version.contains(&v),
            "expected version `{v}` in `{}`",
            tc.version
        );
    }
}

#[test]
#[ignore = "requires network + Erlang/OTP; run via `just uat`"]
fn smoke_build_and_test() {
    let Some(tc) = shared() else {
        eprintln!("skipped: requested rolling release is not available yet");
        return;
    };
    let project = stage_project("smoke");

    let out = tc
        .command()
        .arg("test")
        .current_dir(&project)
        .output()
        .expect("spawn `beamtalk test`");

    assert!(
        out.status.success(),
        "`beamtalk test` failed:\n{}",
        combined(&out)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("0 failed") && stdout.contains("passed"),
        "expected a passing BUnit run, got:\n{}",
        combined(&out)
    );
}
