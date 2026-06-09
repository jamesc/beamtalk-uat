#!/usr/bin/env bash
# Copyright 2026 James Casey
# SPDX-License-Identifier: Apache-2.0
#
# SessionStart hook: bootstrap the cloud environment once per machine.
#
# Runs scripts/setup-cloud.sh on the first session (a marker file prevents
# re-runs). The setup script is idempotent — it skips any tool already present —
# so re-running is cheap and safe; the marker just avoids the apt round-trip on
# every session start.

set -euo pipefail

MARKER="${HOME}/.beamtalk-uat-cloud-setup-done"
if [[ -f "${MARKER}" ]]; then
  exit 0
fi

SETUP_SCRIPT="${CLAUDE_PROJECT_DIR:-${PWD}}/scripts/setup-cloud.sh"
if [[ -f "${SETUP_SCRIPT}" ]]; then
  echo "First session — running UAT cloud environment setup..."
  bash "${SETUP_SCRIPT}"
  touch "${MARKER}"
else
  echo "warning: ${SETUP_SCRIPT} not found — skipping cloud setup" >&2
fi
