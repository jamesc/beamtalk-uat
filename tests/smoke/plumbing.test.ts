// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
//
// Smoke: proves the plumbing end to end against the real released bundle
// (install -> isolated env -> run -> transcript -> teardown). It is NOT a
// contract or journey test; those arrive with BT-5305 and later.

import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test } from '../../lib/index.ts';

test('the installed bundle reports a version', { tag: '@max-otp' }, async ({ uat }) => {
  await uat.step('beamtalk --version', async () => {
    const r = await uat.bt(['--version']);
    expect(r.code, r.stderr).toBe(0);
    expect(r.stdout).toMatch(/\d+\.\d+\.\d+/);
  });
});

test('the harness env is isolated from the runner', { tag: '@max-otp' }, async ({ uat }) => {
  await uat.step('HOME is a throwaway directory', async () => {
    const env = uat.env.childEnv();
    expect(env.HOME).toBe(uat.env.home);
    expect(env.USERPROFILE).toBe(uat.env.home);
    expect(env.GH_TOKEN).toBeUndefined();
    expect(env.GITHUB_TOKEN).toBeUndefined();
  });
});

test('beamtalk new scaffolds a project that builds', { tag: '@max-otp' }, async ({ uat }) => {
  await uat.step('beamtalk new', async () => {
    const r = await uat.bt(['new', 'hello']);
    expect(r.code, r.stderr).toBe(0);
    expect(existsSync(join(uat.env.work, 'hello', 'beamtalk.toml'))).toBe(true);
  });
  await uat.step('beamtalk build', async () => {
    const r = await uat.bt(['build'], { cwd: join(uat.env.work, 'hello'), timeoutMs: 180_000 });
    expect(r.code, r.stdout + r.stderr).toBe(0);
  }, { timeoutMs: 200_000 });
});
