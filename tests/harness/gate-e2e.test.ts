// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
//
// The harness tested end to end by itself: resolve -> install from the run
// artifact -> run deliberately red tests -> check what the report says. These
// guard the three promises of ADR § 5: a red test ships a failure bundle, a
// leftover process or epmd registration is a red step, a hung step is named.
// POSIX only (the fake toolchain is a script); excluded on win32 in the config.

import { expect, test } from '@playwright/test';
import { execFile } from 'node:child_process';
import { chmod, mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { installFromManifest } from '../../lib/toolchain.ts';
import { inputFromEvent, resolve } from '../../lib/resolve.ts';
import { ArtifactPayload } from '../../schemas/index.ts';
import { hostTarget } from '../../lib/targets.ts';
import { FakeSource, payloadFor } from './fake-source.ts';
import { readFile } from 'node:fs/promises';

const exec = promisify(execFile);
const ROOT = join(import.meta.dirname, '..', '..');

async function fakeBundle(): Promise<Buffer> {
  const dir = await mkdtemp(join(tmpdir(), 'btuat-fakebundle-'));
  const top = join(dir, 'beamtalk-1.0.0');
  await mkdir(join(top, 'bin'), { recursive: true });
  await writeFile(
    join(top, 'bin', 'beamtalk'),
    `#!/usr/bin/env node
const a = process.argv.slice(2);
if (a[0] === '--version') console.log('beamtalk 1.0.0');
else if (a[0] === 'doctor') console.log('doctor: all good (fake)');
else { console.error('fake beamtalk: unsupported ' + a.join(' ')); process.exit(2); }
`,
  );
  // A fake epmd driven by a file in the test's private HOME.
  await writeFile(
    join(top, 'bin', 'epmd'),
    `#!/usr/bin/env node
const fs = require('fs'), f = process.env.HOME + '/fake-epmd-names';
const read = () => { try { return fs.readFileSync(f, 'utf8'); } catch { return ''; } };
if (process.argv[2] === '-names') console.log('epmd: up and running on port 4369 with data:\\n' + read());
else if (process.argv[2] === '-stop') fs.writeFileSync(f, '');
`,
  );
  await chmod(join(top, 'bin', 'beamtalk'), 0o755);
  await chmod(join(top, 'bin', 'epmd'), 0o755);
  const tgz = join(dir, 'bundle.tar.gz');
  await exec('tar', ['-czf', tgz, '-C', dir, 'beamtalk-1.0.0']);
  return readFile(tgz);
}

async function resolvedFakeRelease(): Promise<{ dir: string; archive: string }> {
  const target = hostTarget();
  const archive = `beamtalk-1.0.0-${target}.tar.gz`;
  const assets = { [archive]: await fakeBundle() };
  const dir = await mkdtemp(join(tmpdir(), 'btuat-art-'));
  const payload = ArtifactPayload.parse(payloadFor('edge', assets));
  await resolve({ kind: 'payload', payload }, new FakeSource({ edge: assets }), dir);
  return { dir, archive };
}

test('install from the resolve artifact; a corrupted artifact is refused', async () => {
  const { dir, archive } = await resolvedFakeRelease();
  const prefix = join(await mkdtemp(join(tmpdir(), 'btuat-inst-')), 'bt');
  const info = await installFromManifest(dir, hostTarget(), prefix);
  expect(info.version).toBe('beamtalk 1.0.0');
  expect(info.bin).toBe(join(prefix, 'bin', 'beamtalk'));

  await writeFile(join(dir, 'assets', archive), 'bit rot between resolve and the shard');
  await expect(installFromManifest(dir, hostTarget(), prefix + '2')).rejects.toThrow(/artifact corrupted/);
});

interface NestedResult {
  status: string;
  errors: string[];
  attachments: string[];
}

test.describe('the deliberately red tests', () => {
  let results: Record<string, NestedResult>;

  test.beforeAll(async () => {
    const { dir } = await resolvedFakeRelease();
    const scratch = await mkdtemp(join(tmpdir(), 'btuat-nested-'));
    const env = {
      ...process.env,
      BEAMTALK_UAT_ARTIFACT_DIR: dir,
      BEAMTALK_UAT_DIR: join(scratch, 'uat'),
      NESTED_OUT: join(scratch, 'out'),
      BEAMTALK_UAT_VERSION: 'edge',
      CI: '',
    };
    const r = await exec(
      process.execPath,
      [join(ROOT, 'node_modules', '@playwright', 'test', 'cli.js'), 'test', '-c', 'tests/harness/nested/nested.config.ts'],
      { cwd: ROOT, env, maxBuffer: 64 * 1024 * 1024 },
    ).catch((e: { stdout: string }) => e); // exits 1: some tests are meant to be red
    const report = JSON.parse((r as { stdout: string }).stdout) as {
      suites: { specs?: unknown[]; suites?: unknown[] }[];
    };
    results = {};
    const walk = (suites: unknown[]) => {
      for (const s of suites as { specs?: { title: string; tests: { results: { status: string; errors: { message: string }[]; attachments: { name: string }[] }[] }[] }[]; suites?: unknown[] }[]) {
        for (const spec of s.specs ?? []) {
          const res = spec.tests[0]!.results.at(-1)!;
          results[spec.title.split(' ')[0] + ' ' + spec.title.split(' ')[1]] = {
            status: res.status,
            errors: res.errors.map((e) => e.message),
            attachments: res.attachments.map((a) => a.name),
          };
        }
        walk(s.suites ?? []);
      }
    };
    walk(report.suites);
  });

  test('a clean test passes: no false positives from the leak check', () => {
    expect(results['CLEAN passes']?.status).toBe('passed');
  });

  test('a red test attaches the whole failure bundle', () => {
    const r = results['RED step']!;
    expect(r.status).toBe('failed');
    const want = [
      'bundle/repro.txt',
      'bundle/summary.txt',
      'bundle/process-tree.txt',
      'bundle/epmd-names.txt',
      'bundle/beamtalk-version.txt',
      'bundle/beamtalk-doctor.txt',
    ];
    for (const w of want) expect(r.attachments, w).toContain(w);
    expect(r.attachments.some((a) => a.startsWith('bundle/transcripts/'))).toBe(true);
    expect(r.attachments.some((a) => a.startsWith('bundle/crash/') && a.endsWith('erl_crash.dump'))).toBe(true);
    expect(r.attachments.some((a) => a.startsWith('bundle/logs/') && a.endsWith('workspace.log'))).toBe(true);
  });

  test('a process the test never stopped is a red teardown step', () => {
    const r = results['LEAK tracked']!;
    expect(r.status).toBe('failed');
    expect(r.errors.join('\n')).toMatch(/process \d+ \(forgotten\) still running/);
  });

  test('a detached process left in the sandbox is a red teardown step', () => {
    const r = results['LEAK detached']!;
    expect(r.status).toBe('failed');
    expect(r.errors.join('\n')).toMatch(/process \d+ left behind/);
  });

  test('an epmd registration left behind is a red teardown step', () => {
    const r = results['LEAK epmd']!;
    expect(r.status).toBe('failed');
    expect(r.errors.join('\n')).toMatch(/epmd registration left behind: u[0-9a-f]+_web1/);
  });

  test('a hung step is cut off and named', () => {
    const r = results['HANG a']!;
    expect(r.status).toBe('failed');
    expect(r.errors.join('\n')).toContain('step `rpc` exceeded');
  });
});
