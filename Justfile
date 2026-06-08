# Copyright 2026 James Casey
# SPDX-License-Identifier: Apache-2.0

# List available recipes
default:
    @just --list

# Build the harness (compile only, no network/Erlang needed)
build:
    cargo build --all-targets

# Fast unit tests (spec parsing etc. — no toolchain install)
test:
    cargo test --lib

# Run the full UAT suite against a released toolchain version (default: latest).
# Examples:  just uat            (latest release)
#            just uat v0.4.0     (a specific release)
#            just uat nightly    (the rolling nightly)
uat version="latest":
    BEAMTALK_UAT_VERSION={{version}} cargo test --tests -- --ignored --nocapture

# Run the UAT suite against an already-installed `beamtalk` binary (local dev).
uat-local bin:
    BEAMTALK_UAT_BIN={{bin}} cargo test --tests -- --ignored --nocapture
