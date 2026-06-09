#!/bin/bash
# Copyright 2026 James Casey
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

# Beamtalk UAT — Cloud Development Environment Setup
#
# The UAT repo is a *consumer* of Beamtalk: it installs a released toolchain
# bundle (the artifact a user gets) and drives it. It never builds Beamtalk from
# source, so this setup is deliberately minimal compared to the beamtalk repo's
# scripts/setup-cloud.sh — it installs only what's needed to run the UAT suite:
#
#   * Erlang/OTP 27 — required by `beamtalk build`/`run`/`test` (the bunit/run/
#     cli_build scenarios). The released bundle does NOT ship a BEAM runtime; it
#     relies on `erl`/`erlc` on PATH.
#   * just            — the `just uat` entrypoint.
#
# Rust/cargo, gh, tar/unzip and tmux are assumed present (the cloud image and
# the offline CLI/LSP scenarios need them); the script skips anything already
# installed. rebar3 is NOT installed: the release bundle ships its own
# `tools/rebar3` and no UAT scenario pulls hex dependencies.
#
# Usage:
#   ./scripts/setup-cloud.sh
#
# Environment variables:
#   SKIP_ERLANG     - set to 1 to skip Erlang installation
#   SKIP_JUST       - set to 1 to skip `just` installation
#
# Note on the LiveView IDE: it is NOT installed here, and intentionally so.
# The Phoenix `bt_attach` app (beamtalk/editors/liveview) is currently a
# source-only Mix project — it is neither part of the released toolchain bundle
# nor published as a standalone artifact, so there is nothing for UAT to drive
# as a consumer yet. When LiveView UAT lands, the gate-consistent path is to
# publish it as a `mix release` OTP tarball (self-contained: ERTS + all compiled
# .beam, including Elixir's stdlib). UAT would then install and run that release
# with NO Elixir on the host — building it stays in beamtalk's CI. So this
# script deliberately installs no Elixir/Mix toolchain.

# --- Helpers ---

if [ -t 1 ]; then
  GREEN='\033[0;32m'
  YELLOW='\033[1;33m'
  RED='\033[0;31m'
  BOLD='\033[1m'
  NC='\033[0m'
else
  GREEN='' YELLOW='' RED='' BOLD='' NC=''
fi

info()  { echo -e "${BOLD}==> $1${NC}"; }
ok()    { echo -e "  ${GREEN}✓${NC} $1"; }
warn()  { echo -e "  ${YELLOW}!${NC} $1"; }
fail()  { echo -e "  ${RED}✗${NC} $1"; }

have() { command -v "$1" &>/dev/null; }

# Compute SUDO prefix — defer failure until a command actually needs it, so the
# script succeeds when everything is pre-installed even without sudo.
if [ "$(id -u)" -eq 0 ]; then
  SUDO=""
elif have sudo; then
  SUDO="sudo"
else
  SUDO=""
  _NEED_SUDO_WARNING=1
fi

require_sudo() {
  if [ "${_NEED_SUDO_WARNING:-}" = "1" ]; then
    fail "Root privileges required but sudo not available"
    exit 1
  fi
}

# Detect OS
if [ -f /etc/os-release ]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  OS_ID="${ID:-unknown}"
  OS_VERSION="${VERSION_CODENAME:-${VERSION_ID:-unknown}}"
else
  OS_ID="unknown"
  OS_VERSION="unknown"
fi

echo ""
info "Beamtalk UAT cloud environment setup"
echo "  OS: ${OS_ID} ${OS_VERSION}"
echo ""

# --- Erlang/OTP 27 ---
# Mirrors beamtalk/scripts/setup-cloud.sh so UAT runs against the same OTP the
# toolchain is released for.

if [ "${SKIP_ERLANG:-}" = "1" ]; then
  warn "Skipping Erlang (SKIP_ERLANG=1)"
elif have erl; then
  OTP_VSN=$(erl -eval 'io:format("~s",[erlang:system_info(otp_release)]),halt().' -noshell 2>/dev/null || echo "unknown")
  ok "Erlang/OTP already installed (OTP ${OTP_VSN})"
else
  info "Installing Erlang/OTP 27..."
  require_sudo
  case "$OS_ID" in
    ubuntu|debian)
      $SUDO apt-get update -qq
      $SUDO apt-get install -y -qq --no-install-recommends ca-certificates gnupg curl
      $SUDO mkdir -p /etc/apt/keyrings
      # Remove any stale erlang-solutions list from a previous partial run
      $SUDO rm -f /etc/apt/sources.list.d/erlang-solutions.list
      curl -fsSL --retry 5 --retry-connrefused --retry-delay 2 \
        https://binaries2.erlang-solutions.com/GPG-KEY-pmanager.asc \
        -o /tmp/GPG-KEY-pmanager.asc
      $SUDO rm -f /etc/apt/keyrings/erlang-solutions.gpg
      gpg --batch --dearmor -o /tmp/erlang-solutions.gpg /tmp/GPG-KEY-pmanager.asc
      $SUDO mv /tmp/erlang-solutions.gpg /etc/apt/keyrings/erlang-solutions.gpg
      rm -f /tmp/GPG-KEY-pmanager.asc
      # Erlang Solutions only publishes Debian codenames (not Ubuntu ones).
      # Map Ubuntu codenames to the closest Debian base.
      case "${VERSION_CODENAME:-}" in
        noble|jammy|focal) CODENAME="bookworm" ;;
        mantic|lunar)      CODENAME="bookworm" ;;
        *)                 CODENAME="${VERSION_CODENAME:-bookworm}" ;;
      esac
      echo "deb [signed-by=/etc/apt/keyrings/erlang-solutions.gpg] https://binaries2.erlang-solutions.com/debian/ ${CODENAME}-esl-erlang-27 contrib" \
        | $SUDO tee /etc/apt/sources.list.d/erlang-solutions.list > /dev/null
      $SUDO apt-get update -qq
      $SUDO apt-get install -y -qq --no-install-recommends esl-erlang
      ok "Erlang/OTP 27 installed"
      ;;
    *)
      fail "Unsupported OS for Erlang installation: ${OS_ID}"
      fail "Install Erlang/OTP 27 manually and re-run with SKIP_ERLANG=1"
      exit 1
      ;;
  esac
fi

# --- just ---
# The `just uat` entrypoint. Installed into ~/.cargo/bin (already on PATH when
# cargo is present) so no sudo is needed.

if [ "${SKIP_JUST:-}" = "1" ]; then
  warn "Skipping just (SKIP_JUST=1)"
elif have just; then
  ok "just already installed ($(just --version))"
else
  info "Installing just..."
  JUST_DEST="${CARGO_HOME:-$HOME/.cargo}/bin"
  mkdir -p "${JUST_DEST}"
  curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
    | bash -s -- --to "${JUST_DEST}"
  ok "just installed to ${JUST_DEST}"
fi

# --- Verify ---

echo ""
info "Verifying installations..."
ERRORS=0

# Required for the full UAT suite.
REQUIRED="rustc cargo erl gh just tar"
[ "${SKIP_ERLANG:-}" = "1" ] && REQUIRED="${REQUIRED/erl/}"
[ "${SKIP_JUST:-}" = "1" ] && REQUIRED="${REQUIRED/just/}"
for cmd in $REQUIRED; do
  if have "$cmd"; then
    ok "$cmd"
  else
    fail "$cmd NOT FOUND"
    ERRORS=$((ERRORS + 1))
  fi
done

# Optional — informational only.
for cmd in tmux unzip; do
  if have "$cmd"; then ok "$cmd (optional)"; else warn "$cmd not installed (optional)"; fi
done

echo ""
if [ "$ERRORS" -gt 0 ]; then
  fail "${ERRORS} required tool(s) failed to install"
  exit 1
else
  info "UAT environment ready."
  echo ""
  echo "  Next steps:"
  echo "    just uat                # install the latest release and run the suite"
  echo "    just uat-local <bin>    # run against an already-installed binary"
  echo ""
fi
