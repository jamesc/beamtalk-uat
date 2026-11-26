// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
//
// Tests of proc.ts / env.ts. Each guards a failure the gate would otherwise miss:
// a grandchild surviving teardown, a hung step, two tests sharing a port or node.

import { expect, test } from '@playwright/test';
import { UatEnv, allocatePort } from '../../lib/env.ts';
import {
  Proc,
  Transcript,
  descendantsOf,
  isAlive,
  killTree,
  listProcesses,
  parseEpmdNames,
  run,
  sleep,
  withHangGuard,
} from '../../lib/proc.ts';
import { mkdtemp, readFile, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const node = process.execPath;
// Parent that starts a grandchild, prints the grandchild's pid, then idles.
const PARENT = `
const { spawn } = require('node:child_process');
const kid = spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { stdio: 'ignore' });
console.log('GRANDCHILD ' + kid.pid);
setInterval(()=>{},1000);
`;

test('killTree kills grandchildren, not just the direct child', async () => {
  const p = await Proc.spawn(node, ['-e', PARENT]);
  const [, gc] = (await p.waitForOutput(/GRANDCHILD (\d+)/)).map(Number) as [number, number];
  expect(isAlive(gc)).toBe(true);
  await p.stop();
  await sleep(200);
  expect(p.exited).toBe(true);
  expect(isAlive(gc), 'grandchild survived killTree').toBe(false);
});

test('descendantsOf finds nested children', async () => {
  const p = await Proc.spawn(node, ['-e', PARENT]);
  const [, gc] = (await p.waitForOutput(/GRANDCHILD (\d+)/)).map(Number) as [number, number];
  const kids = descendantsOf(p.pid!, await listProcesses()).map((k) => k.pid);
  expect(kids).toContain(gc);
  await p.stop();
});

test('run() times out, kills the tree and says so', async () => {
  const r = await run(node, ['-e', 'setInterval(()=>{},1000)'], { timeoutMs: 400 });
  expect(r.timedOut).toBe(true);
});

test('run() captures streams and exit code separately', async () => {
  const r = await run(node, ['-e', 'console.log("out"); console.error("err"); process.exit(3)']);
  expect(r).toMatchObject({ code: 3, stdout: 'out\n', stderr: 'err\n', timedOut: false });
});

test('readiness: waitForOutput times out with the output tail, and fails fast on early exit', async () => {
  const slow = await Proc.spawn(node, ['-e', 'console.log("booting"); setInterval(()=>{},1000)']);
  await expect(slow.waitForOutput(/READY/, 500)).rejects.toThrow(/no output matching.*within 500ms[\s\S]*booting/);
  await slow.stop();
  const dead = await Proc.spawn(node, ['-e', 'console.log("bye")']);
  await expect(dead.waitForOutput(/READY/, 5_000)).rejects.toThrow(/exited \(code 0\) before output matched/);
});

test('readiness: waitForPort sees a listener', async () => {
  const port = await allocatePort();
  const p = await Proc.spawn(node, ['-e', `require('net').createServer().listen(${port},'127.0.0.1');console.log('up')`]);
  await p.waitForPort(port, '127.0.0.1', 10_000);
  await p.stop();
});

test('the hang guard names the step and the limit', async () => {
  await expect(withHangGuard('rpc', 300, () => new Promise(() => {}))).rejects.toThrow('step `rpc` exceeded 0s (hang guard)');
  await expect(withHangGuard('rpc', 1500, () => new Promise(() => {}))).rejects.toThrow('step `rpc` exceeded 2s');
  await expect(withHangGuard('quick', 1000, async () => 7)).resolves.toBe(7);
});

test('transcripts: one numbered file per step with command, output and exit', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'btuat-tx-'));
  const t = new Transcript(dir);
  await t.beginStep('first step');
  await run(node, ['-e', 'console.log("hello")'], { transcript: t });
  await t.beginStep('second: with/odd chars');
  await run(node, ['-e', 'process.exit(2)'], { transcript: t });
  const files = (await readdir(dir)).sort();
  expect(files).toEqual(['01-first-step.log', '02-second-with-odd-chars.log']);
  const first = await readFile(join(dir, files[0]!), 'utf8');
  expect(first).toContain('hello');
  expect(first).toContain('[exit] code=0');
  expect(await readFile(join(dir, files[1]!), 'utf8')).toContain('[exit] code=2');
});

test('parseEpmdNames', () => {
  const out = 'epmd: up and running on port 4369 with data:\nname u1_n1 at port 40001\nname other at port 40002\n';
  expect(parseEpmdNames(out)).toEqual(['u1_n1', 'other']);
  expect(parseEpmdNames('epmd: Cannot connect to local epmd')).toEqual([]);
});

test.describe('env isolation', () => {
  test('HOME/USERPROFILE are private and runner secrets are stripped', async () => {
    process.env.GH_TOKEN = 'secret';
    process.env.BEAMTALK_SOMETHING = '1';
    process.env.RELEASE_COOKIE = 'c';
    process.env.ERL_AFLAGS = '-x';
    const env = await UatEnv.create();
    try {
      const c = env.childEnv();
      expect(c.HOME).toBe(env.home);
      expect(c.USERPROFILE).toBe(env.home);
      expect(c.HOME).not.toBe(process.env.HOME);
      for (const k of ['GH_TOKEN', 'BEAMTALK_SOMETHING', 'RELEASE_COOKIE', 'ERL_AFLAGS']) expect(c[k], k).toBeUndefined();
      expect(env.childEnv({ RELEASE_COOKIE: 'explicit' }).RELEASE_COOKIE).toBe('explicit');
    } finally {
      await env.dispose();
      for (const k of ['GH_TOKEN', 'BEAMTALK_SOMETHING', 'RELEASE_COOKIE', 'ERL_AFLAGS']) delete process.env[k];
    }
  });

  test('ports, node names and cookies never repeat across tests', async () => {
    const [a, b] = await Promise.all([UatEnv.create(), UatEnv.create()]);
    try {
      expect(a.id).not.toBe(b.id);
      const ports = await Promise.all(Array.from({ length: 40 }, () => allocatePort()));
      expect(new Set(ports).size).toBe(40);
      const names = [a.nodeName(), a.nodeName(), b.nodeName(), b.nodeName('Web Node')];
      expect(new Set(names).size).toBe(4);
      for (const n of names) expect(n).toMatch(/^[a-z0-9_]+$/);
      expect(new Set([a.cookie(), a.cookie(), b.cookie()]).size).toBe(3);
    } finally {
      await a.dispose();
      await b.dispose();
    }
  });

  test('the toolchain bin dir is first on PATH', async () => {
    const env = await UatEnv.create({ bin: join('/opt', 'bt', 'bin', 'beamtalk') });
    try {
      const key = Object.keys(env.childEnv()).find((k) => k.toUpperCase() === 'PATH')!;
      expect(env.childEnv()[key]!.startsWith(join('/opt', 'bt', 'bin'))).toBe(true);
    } finally {
      await env.dispose();
    }
  });
});
