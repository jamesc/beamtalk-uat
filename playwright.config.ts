// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
//
// OWNER-GATED (ADR 0154 § 2): do not widen timeouts or add retries to make CI green.

import { defineConfig } from '@playwright/test';

const CI = !!process.env.CI;
const shardLabel = process.env.BEAMTALK_UAT_LEG ?? 'local';

export default defineConfig({
  testDir: 'tests',
  outputDir: 'test-results',
  // A whole run must finish well inside the workflow's job timeout.
  globalTimeout: 25 * 60_000,
  // Per-test budget. Individual steps carry their own, tighter hang guards.
  timeout: 5 * 60_000,
  expect: { timeout: 10_000 },
  // No automatic retries (ADR § 11): a test that passes on a rerun is `flaky`, not green.
  retries: 0,
  forbidOnly: CI,
  fullyParallel: true,
  workers: CI ? 2 : undefined,
  reporter: [
    ['list'],
    ['html', { open: 'never', outputFolder: 'playwright-report' }],
    ['junit', { outputFile: `test-results/junit-${shardLabel}.xml` }],
    ['json', { outputFile: 'test-results/report.json' }],
  ],
  use: { trace: 'off' },
  projects: [
    // Hermetic tests OF the harness. No toolchain, no network.
    {
      name: 'harness',
      testDir: 'tests/harness',
      testMatch: '*.test.ts',
      // The nested-run test drives a POSIX script as a fake toolchain.
      testIgnore: process.platform === 'win32' ? ['**/gate-e2e.test.ts'] : [],
    },
    // Plumbing smoke against the real released bundle. The toolchain is installed
    // by scripts/prepare-toolchain.ts before this runs (`npm run uat`, or the
    // workflow's install step), so every shard sees the same install.
    {
      name: 'smoke',
      testDir: 'tests/smoke',
      testMatch: '*.test.ts',
    },
  ],
});
