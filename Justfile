# Copyright 2026 James Casey
# SPDX-License-Identifier: Apache-2.0

# List available recipes
default:
    @just --list

# Install the harness's dependencies (exact versions from package-lock.json)
install:
    npm ci

# Typecheck the harness
typecheck:
    npm run typecheck

# The harness's tests of itself (hermetic: no toolchain, no network)
test:
    npx playwright test --project=harness

# Run the gate against a released toolchain: latest | edge | nightly | vX.Y.Z | local:<path>.
# Examples:  just uat              (latest release)
#            just uat v0.4.0       (a specific release)
#            just uat edge         (the rolling edge pre-release)
uat version="latest":
    BEAMTALK_UAT_VERSION={{version}} npm run uat

# Run against an already-installed `beamtalk` binary (local dev).
uat-local bin:
    BEAMTALK_UAT_VERSION=local:{{bin}} npm run uat

# ── Legacy Rust harness ──────────────────────────────────────────────────
# Kept only until BT-5305's J1/J2 and contracts are green on all four targets
# against `latest`; the PR that switches uat.yml deletes these and src/, tests/*.rs,
# Cargo.*, projects/.

legacy-build:
    cargo build --all-targets

legacy-test:
    cargo test --lib

legacy-uat version="latest":
    BEAMTALK_UAT_VERSION={{version}} cargo test --tests -- --ignored --nocapture

legacy-uat-local bin:
    BEAMTALK_UAT_BIN={{bin}} cargo test --tests -- --ignored --nocapture
