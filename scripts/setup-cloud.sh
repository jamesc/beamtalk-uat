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
#   * Erlang/OTP — required by `beamtalk build`/`run`/`test` (the bunit/run/
#     cli_build scenarios). The released bundle does NOT ship a BEAM runtime; it
#     relies on `erl`/`erlc` on PATH. The version is pinned by the repo-root
#     .tool-versions (OTP 28.5 at time of writing) — the SAME mechanism CI's
#     setup-beam uses (version-file: .tool-versions) and the SAME mechanism the
#     beamtalk repo uses, so the OTP we install never drifts from what the
#     released bundle is built against. (Previously this script apt-installed a
#     hand-pinned OTP 27, which silently fell behind once beamtalk moved to
#     OTP 28.5.)
#   * just            — the `just uat` entrypoint.
#
# Erlang is installed via mise (precompiled BEAM builds — no source compile),
# exactly like the beamtalk repo's setup. We never apt-install erlang.
#
# Rust/cargo, gh, tar/unzip and tmux are assumed present (the cloud image and
# the offline CLI/LSP scenarios need them); the script skips anything already
# installed. rebar3 is NOT installed: the release bundle ships its own
# `tools/rebar3` and no UAT scenario pulls hex dependencies. (CI's setup-beam
# still reads the `rebar` pin from .tool-versions, so the CI legs match.)
#
# Usage:
#   ./scripts/setup-cloud.sh
#
# Environment variables:
#   MISE_VERSION    - mise version to install (default: v2026.6.3, matches the
#                     beamtalk devcontainer)
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

MISE_VERSION="${MISE_VERSION:-v2026.6.3}"
# mise installs the world-readable toolchain here (mirrors the beamtalk
# Dockerfile's MISE_DATA_DIR) so its shims can sit on PATH for every shell.
MISE_DATA_DIR="${MISE_DATA_DIR:-/usr/local/share/mise}"
export MISE_DATA_DIR
MISE_SHIMS="${MISE_DATA_DIR}/shims"

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

# Repo root (the directory containing .tool-versions). Resolution order mirrors
# the beamtalk repo's setup-cloud.sh:
#   1. CLAUDE_PROJECT_DIR if the harness exports it (the checkout the hook runs in)
#   2. the script's own parent dir — only when run from a file on disk
#   3. the current working directory — the `curl … | bash` case, where the
#      script arrives on stdin and BASH_SOURCE is unset (so guard it under set -u)
# Then, if .tool-versions isn't at the guessed root, locate the checkout: walk
# up from the cwd, then look one level down (the cloud builds the environment
# from a parent dir with the checkout in a child).
if [ -n "${CLAUDE_PROJECT_DIR:-}" ]; then
  REPO_ROOT="${CLAUDE_PROJECT_DIR}"
elif [ -n "${BASH_SOURCE[0]:-}" ]; then
  REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
else
  REPO_ROOT="$(pwd)"
fi
if [ ! -f "${REPO_ROOT}/.tool-versions" ]; then
  _guess="${REPO_ROOT}"
  # (a) Walk up from the cwd.
  _dir="$(pwd)"
  while [ "${_dir}" != "/" ]; do
    if [ -f "${_dir}/.tool-versions" ]; then
      REPO_ROOT="${_dir}"
      break
    fi
    _dir="$(dirname "${_dir}")"
  done
  # (b) Not at/above the cwd — look one level down into the cwd's and $HOME's
  #     children (the cloud's checkout-in-a-subdir layout). The [ -f ] guard
  #     handles the no-match case where the glob stays literal.
  if [ ! -f "${REPO_ROOT}/.tool-versions" ]; then
    for _cand in "$(pwd)"/*/.tool-versions "${HOME:-/root}"/*/.tool-versions; do
      [ -f "${_cand}" ] || continue
      REPO_ROOT="$(cd "$(dirname "${_cand}")" && pwd)"
      break
    done
  fi
  # Surface the reroute — otherwise a wrong .tool-versions is impossible to
  # trace back from the resolved toolchain pins.
  [ "${REPO_ROOT}" = "${_guess}" ] || warn "REPO_ROOT resolved to ${REPO_ROOT} (searched from $(pwd))"
fi
TOOL_VERSIONS="${REPO_ROOT}/.tool-versions"

# Last resort: no checkout on disk anywhere we looked (e.g. the cloud builds the
# environment snapshot before the repo is cloned). Fetch the canonical pins from
# the default branch so mise can still resolve the BEAM toolchain.
if [ ! -f "${TOOL_VERSIONS}" ]; then
  _PINS_REF="${BEAMTALK_UAT_PINS_REF:-main}"
  _PINS_DIR="$(mktemp -d)"
  if curl -fsSL --retry 5 --retry-connrefused --retry-delay 2 --connect-timeout 20 \
       "https://raw.githubusercontent.com/jamesc/beamtalk-uat/${_PINS_REF}/.tool-versions" \
       -o "${_PINS_DIR}/.tool-versions" 2>/dev/null && [ -s "${_PINS_DIR}/.tool-versions" ]; then
    REPO_ROOT="${_PINS_DIR}"
    TOOL_VERSIONS="${REPO_ROOT}/.tool-versions"
    warn "No checkout found — using BEAM pins from origin/${_PINS_REF}"
  fi
fi

# Trust the checkout for mise so .tool-versions always resolves the pinned BEAM
# toolchain regardless of mise's snapshot-fragile trust store. Persisted to
# /etc/profile.d below so every future shell trusts it too.
case ":${MISE_TRUSTED_CONFIG_PATHS:-}:" in
  *":${REPO_ROOT}:"*) ;;
  *) export MISE_TRUSTED_CONFIG_PATHS="${REPO_ROOT}${MISE_TRUSTED_CONFIG_PATHS:+:${MISE_TRUSTED_CONFIG_PATHS}}" ;;
esac

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

# --- Erlang/OTP via mise (pinned by .tool-versions) ---
#
# Pinned by repo-root .tool-versions (OTP 28.5 at time of writing) — the single
# source of truth shared with CI's setup-beam (version-file: .tool-versions) and
# the beamtalk repo. mise fetches precompiled BEAM builds — no source compile.

if [ "${SKIP_ERLANG:-}" = "1" ]; then
  warn "Skipping Erlang (SKIP_ERLANG=1)"
elif [ ! -f "${TOOL_VERSIONS}" ]; then
  fail "No .tool-versions at ${TOOL_VERSIONS} — cannot resolve the OTP pin"
  exit 1
else
  ERLANG_PIN="$(awk '/^erlang[[:space:]]/{print $2; exit}' "${TOOL_VERSIONS}")"

  # 1. mise itself.
  if have mise; then
    MISE_BIN="$(command -v mise)"
    ok "mise already installed ($("${MISE_BIN}" --version 2>/dev/null | head -1))"
  elif [ -x /usr/local/bin/mise ]; then
    MISE_BIN=/usr/local/bin/mise
    ok "mise already installed ($("${MISE_BIN}" --version 2>/dev/null | head -1))"
  else
    info "Installing mise ${MISE_VERSION}..."
    curl -fsSL --retry 5 --retry-connrefused --retry-delay 2 --connect-timeout 20 https://mise.run \
      | MISE_VERSION="${MISE_VERSION}" MISE_INSTALL_PATH=/usr/local/bin/mise sh
    MISE_BIN=/usr/local/bin/mise
    ok "mise installed"
  fi

  # Record trust in mise's state dir too (belt-and-suspenders alongside
  # MISE_TRUSTED_CONFIG_PATHS) so a bare `mise` invocation still resolves the pin.
  "${MISE_BIN}" trust "${REPO_ROOT}" >/dev/null 2>&1 || true

  # A GITHUB_TOKEN lifts the unauthenticated GitHub API rate limit mise hits
  # while resolving release lists; it degrades gracefully without one.
  export GITHUB_TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

  # mise's precompiled OTP links against system libssl + libncurses at RUNTIME.
  # A bare cloud base image may not ship them, so `erl` extracts fine yet fails
  # to start with a shared-library loader error. Install the deps up front.
  if command -v apt-get >/dev/null 2>&1; then
    _OTP_DEPS=""
    for pkg in libssl-dev libncurses-dev; do
      dpkg -s "$pkg" >/dev/null 2>&1 || _OTP_DEPS="${_OTP_DEPS} $pkg"
    done
    if [ -n "${_OTP_DEPS}" ]; then
      info "Installing Erlang runtime deps (${_OTP_DEPS# })..."
      require_sudo
      # shellcheck disable=SC2086
      if $SUDO apt-get update -qq && $SUDO apt-get install -y -qq --no-install-recommends ${_OTP_DEPS}; then
        ok "Erlang runtime deps installed"
      else
        warn "Could not install Erlang runtime deps (${_OTP_DEPS# }) — erl may fail to start"
      fi
    fi
  fi

  # 2. Erlang from .tool-versions.
  info "Installing Erlang/OTP ${ERLANG_PIN:-(from .tool-versions)} via mise — pinned by .tool-versions..."
  (cd "${REPO_ROOT}" && "${MISE_BIN}" install erlang) \
    || warn "mise install reported issues (continuing — erlang may still be present)"
  (cd "${REPO_ROOT}" && "${MISE_BIN}" reshim >/dev/null 2>&1) || true

  # 2b. Pin erlang as a mise GLOBAL default too — not just the project
  #     .tool-versions. A mise shim resolves its version by walking up from the
  #     *current working directory*; with only the project pin, any process that
  #     shells out to `erl` from a directory OUTSIDE the repo finds no pin and
  #     the shim aborts with "No version is set for shim: erl". The global
  #     default carries the same pinned version, so this resolves the
  #     out-of-tree cwd case without bypassing version management.
  [ -n "${ERLANG_PIN}" ] && ("${MISE_BIN}" use -g "erlang@${ERLANG_PIN}" >/dev/null 2>&1 || true)

  # 3. Put the mise shims first on PATH so erl/erlc resolve to the pinned OTP —
  #    both for the rest of this script and (persisted below) every shell.
  case ":${PATH}:" in *":${MISE_SHIMS}:"*) ;; *) export PATH="${MISE_SHIMS}:${PATH}" ;; esac
  hash -r 2>/dev/null || true

  # 3b. Functional smoke test: a precompiled OTP can download cleanly yet fail to
  #     *run* if a runtime lib is still missing. Surface the real loader error
  #     here (the `if` condition exempts this from set -e) instead of letting it
  #     resurface as a cryptic "erl NOT FOUND" at verify.
  if _erl_smoke="$(erl -noshell -eval 'halt(0).' 2>&1)"; then
    ok "erl runs"
  else
    fail "erl installed but cannot start — toolchain will not work:"
    printf '%s\n' "${_erl_smoke}" | sed 's/^/      /'
    warn "Usually a missing runtime library (libssl/libncurses/libtinfo); install the lib it names and re-run."
  fi

  # 4. Persist MISE_DATA_DIR + shims PATH + trust for every future shell. The
  #    setup script's filesystem changes are snapshotted by the cloud cache, so
  #    this lands the toolchain on PATH at session start.
  require_sudo
  $SUDO tee /etc/profile.d/beamtalk-uat-mise.sh > /dev/null << PROFILE
# Beamtalk UAT BEAM toolchain (managed by scripts/setup-cloud.sh) — do not edit.
export MISE_DATA_DIR="${MISE_DATA_DIR}"
case ":\${PATH}:" in
  *":${MISE_SHIMS}:"*) ;;
  *) export PATH="${MISE_SHIMS}:\${PATH}" ;;
esac
case ":\${MISE_TRUSTED_CONFIG_PATHS:-}:" in
  *":${REPO_ROOT}:"*) ;;
  *) export MISE_TRUSTED_CONFIG_PATHS="${REPO_ROOT}\${MISE_TRUSTED_CONFIG_PATHS:+:\${MISE_TRUSTED_CONFIG_PATHS}}" ;;
esac
PROFILE
  ok "Toolchain PATH persisted to /etc/profile.d/beamtalk-uat-mise.sh"

  # 4b. Symlink the pinned BEAM tools into /usr/local/bin. The /etc/profile.d
  #     export above only reaches LOGIN shells; the agent harness runs commands
  #     in NON-login shells whose PATH it controls directly (it sources neither
  #     /etc/profile.d nor ~/.bashrc). /usr/local/bin IS on that bare PATH, so
  #     linking the shims there is what makes the toolchain resolve for harness
  #     runs. The shims still defer to the .tool-versions pin at run time.
  _linked=()
  for _shim in "${MISE_SHIMS}"/*; do
    [[ -f "${_shim}" && -x "${_shim}" ]] || continue
    _name="$(basename "${_shim}")"
    case "${_name}" in *.ps1) continue ;; esac
    _dest="/usr/local/bin/${_name}"
    # Never clobber a real binary already in /usr/local/bin; only create/refresh
    # symlinks that we own.
    [[ -e "${_dest}" && ! -L "${_dest}" ]] && continue
    $SUDO ln -sf "${_shim}" "${_dest}" && _linked+=("${_name}")
  done
  ok "Linked BEAM tools into /usr/local/bin (${#_linked[@]}): ${_linked[*]:-(none)}"

  # 5. Decommission any stale esl-erlang baked into the cloud base image (or a
  #    previous run of this script, which used to apt-install OTP 27). It ships
  #    an old OTP and installs /usr/bin/erl, which shadows the mise-pinned OTP
  #    for any shell that hasn't sourced the profile.d shims. We install the
  #    BEAM toolchain exclusively via mise now, so purge it and its apt source.
  if dpkg -s esl-erlang &>/dev/null; then
    info "Removing stale esl-erlang (apt OTP) — toolchain is mise-managed now..."
    require_sudo
    $SUDO apt-get purge -y -qq esl-erlang >/dev/null 2>&1 || true
    $SUDO apt-get autoremove -y -qq >/dev/null 2>&1 || true
    $SUDO rm -f /etc/apt/sources.list.d/erlang-solutions.list \
                /etc/apt/keyrings/erlang-solutions.gpg 2>/dev/null || true
    hash -r 2>/dev/null || true
    if dpkg -s esl-erlang &>/dev/null; then
      warn "esl-erlang still present after purge"
    else
      ok "esl-erlang removed"
    fi
  fi
fi

# --- just ---
# The `just uat` entrypoint. Best-effort: a transient just.systems hiccup (it can
# return 403 behind some egress proxies) must NOT abort the whole setup — Erlang
# is the critical piece. Try a few install methods and only warn if all fail.

install_just() {
  local dest="${CARGO_HOME:-$HOME/.cargo}/bin"
  mkdir -p "${dest}"
  # 1) Prebuilt binary from just.systems (fast, no sudo).
  if curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --retry-delay 2 \
        https://just.systems/install.sh 2>/dev/null \
        | bash -s -- --to "${dest}" >/dev/null 2>&1 && have just; then
    return 0
  fi
  # 2) Distro package (Debian/Ubuntu universe ships `just`; needs sudo).
  if { [ "$OS_ID" = "ubuntu" ] || [ "$OS_ID" = "debian" ]; } \
        && [ "${_NEED_SUDO_WARNING:-}" != "1" ] \
        && $SUDO apt-get install -y -qq just >/dev/null 2>&1 && have just; then
    return 0
  fi
  # 3) Build from source via cargo (slow but reliable, no sudo).
  if have cargo && cargo install just --locked >/dev/null 2>&1 && have just; then
    return 0
  fi
  return 1
}

if [ "${SKIP_JUST:-}" = "1" ]; then
  warn "Skipping just (SKIP_JUST=1)"
elif have just; then
  ok "just already installed ($(just --version))"
else
  info "Installing just..."
  if install_just; then
    ok "just installed ($(just --version))"
  else
    warn "Could not install just (tried just.systems, apt, cargo) — continuing."
    warn "Erlang is installed; run the suite directly with:"
    warn "    cargo test --tests -- --ignored --nocapture"
    warn "Install just later for the \`just uat\` shortcut: https://just.systems"
  fi
fi

# --- Verify ---

echo ""
info "Verifying installations..."
ERRORS=0

# Required for the suite (sans the `just` shortcut, which is best-effort above).
REQUIRED="rustc cargo erl gh tar"
[ "${SKIP_ERLANG:-}" = "1" ] && REQUIRED="${REQUIRED/erl/}"
for cmd in $REQUIRED; do
  if have "$cmd"; then
    ok "$cmd"
  else
    fail "$cmd NOT FOUND"
    ERRORS=$((ERRORS + 1))
  fi
done

# Optional / best-effort — informational only.
for cmd in just tmux unzip; do
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
