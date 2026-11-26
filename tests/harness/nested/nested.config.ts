// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
// Config for the harness's tests of ITSELF: runs deliberately-red tests against a
// fake toolchain (see ../gate-e2e.test.ts). Not part of any gate project.
import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  outputDir: process.env.NESTED_OUT ?? '../../../test-results/nested',
  retries: 0,
  workers: 1,
  reporter: [['json']],
  projects: [
    { name: 'setup', testMatch: 'toolchain.setup.ts' },
    { name: 'nested', testMatch: '*.nested.ts', dependencies: ['setup'] },
  ],
});
