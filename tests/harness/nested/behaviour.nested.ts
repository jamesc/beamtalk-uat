// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
// Deliberately red/green tests, run by ../gate-e2e.test.ts to prove the harness
// reports what it must. Titles are matched by the parent test.

import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { expect, test } from '../../../lib/index.ts';

test('CLEAN passes and leaves nothing behind', async ({ uat }) => {
  await uat.step('version', async () => {
    const r = await uat.bt(['--version']);
    expect(r.stdout).toContain('beamtalk 1.0.0');
  });
  const p = await uat.spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { label: 'idle' });
  await p.stop();
});

test('RED step failure attaches a failure bundle', async ({ uat }) => {
  await uat.bt(['--version']);
  await mkdir(join(uat.env.work, 'proj'), { recursive: true });
  await writeFile(join(uat.env.work, 'proj', 'erl_crash.dump'), '=erl_crash_dump:0.5\nSlogan: test\n');
  await writeFile(join(uat.env.home, 'workspace.log'), 'workspace log line\n');
  await uat.spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { label: 'still-running' }).then(() => {});
  await uat.step('the failing step', async () => {
    expect(1 + 1).toBe(3);
  });
});

test('LEAK tracked process never stopped is red', async ({ uat }) => {
  await uat.spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { label: 'forgotten' });
});

test('LEAK detached process in the sandbox is red', async ({ uat }) => {
  await uat.run(process.execPath, [
    '-e',
    `require('child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)','${uat.env.home}'],{detached:true,stdio:'ignore'}).unref()`,
  ]);
});

test('LEAK epmd registration left behind is red', async ({ uat }) => {
  // The fake bundle ships a fake `epmd` that reads this file.
  await writeFile(join(uat.env.home, 'fake-epmd-names'), `name ${uat.env.nodeName('web')} at port 41000\n`);
});

test('HANG a step that never returns trips the guard', async ({ uat }) => {
  await uat.step('rpc', () => new Promise(() => {}), { timeoutMs: 700 });
});
