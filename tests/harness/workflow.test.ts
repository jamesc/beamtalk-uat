// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
//
// Guards on gate.yml's wiring: the properties ADR 0154 § 3 requires are easy to
// lose in a YAML edit and invisible until a release run.

import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const yml = readFileSync(join(import.meta.dirname, '..', '..', '.github', 'workflows', 'gate.yml'), 'utf8');

test('accepts both the new and the legacy dispatch types', () => {
  expect(yml).toMatch(/types:\s*\[beamtalk-artifact,\s*beamtalk-release\]/);
});

test('rolling channels cancel the older run; a release run is never cancelled', () => {
  const group = /group:\s*>-\s*([\s\S]*?)\n\s*cancel-in-progress:\s*>-\s*([\s\S]*?)\n\npermissions:/.exec(yml);
  expect(group, 'concurrency block').not.toBeNull();
  const [, groupExpr, cancelExpr] = group!;
  expect(groupExpr).toContain("'edge'");
  expect(groupExpr).toContain("'nightly'");
  // A release gets its own group per run, so a pending release run is never replaced.
  expect(groupExpr).toContain("format('release-{0}', github.run_id)");
  expect(cancelExpr).toContain("== 'edge'");
  expect(cancelExpr).toContain("== 'nightly'");
  expect(cancelExpr).not.toContain('release');
});

test('every leg installs from the run artifact, and superseded cancels the run', () => {
  expect(yml).toContain('actions/download-artifact');
  expect(yml).toContain('BEAMTALK_UAT_ARTIFACT_DIR');
  expect(yml).toContain('gh run cancel');
  expect(yml).toContain("needs.resolve.outputs.superseded != 'true'");
});

test('runners are the pinned images of ADR § 3 (no floating labels)', () => {
  expect(yml).not.toMatch(/runs-on:.*-latest/);
  const toml = readFileSync(join(import.meta.dirname, '..', '..', 'targets.toml'), 'utf8');
  for (const r of ['ubuntu-24.04', 'macos-15"', 'macos-15-intel', 'windows-2022']) expect(toml).toContain(r);
  expect(toml).not.toMatch(/-latest/);
});
