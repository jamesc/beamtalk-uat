// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * The test API: `import { test, expect } from '../../lib'`.
 *
 * `uat` is the per-test handle. It owns an isolated {@link UatEnv}, records a
 * transcript per step, kills the process tree on teardown, attaches a failure
 * bundle on red and turns anything left running (process or epmd registration)
 * into a red step.
 */
import { test as base, expect } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { ToolchainInfo } from '../schemas/index.ts';
import { buildFailureBundle } from './artifacts.ts';
import { UatEnv } from './env.ts';
import {
  Proc,
  Transcript,
  epmdNames,
  epmdStop,
  findProcessesMentioning,
  isAlive,
  killTree,
  run,
  withHangGuard,
  type RunOptions,
  type RunResult,
} from './proc.ts';
import { loadToolchain } from './toolchain.ts';

export { expect };
export { UatEnv } from './env.ts';
export * from './proc.ts';

export const DEFAULT_STEP_TIMEOUT_MS = 60_000;

export class Uat {
  readonly procs: Proc[] = [];
  constructor(
    readonly env: UatEnv,
    readonly toolchain: ToolchainInfo,
    readonly transcript: Transcript,
  ) {}

  /** Run `beamtalk <args>` in `cwd` (default: the test's work dir). */
  bt(args: string[], opts: Omit<RunOptions, 'env'> & { env?: Record<string, string | undefined> } = {}): Promise<RunResult> {
    return run(this.toolchain.bin, args, {
      cwd: opts.cwd ?? this.env.work,
      env: this.env.childEnv(opts.env),
      timeoutMs: opts.timeoutMs,
      input: opts.input,
      transcript: this.transcript,
    });
  }

  /** Run any command in the isolated environment. */
  run(cmd: string, args: string[], opts: Omit<RunOptions, 'env'> & { env?: Record<string, string | undefined> } = {}): Promise<RunResult> {
    return run(cmd, args, {
      cwd: opts.cwd ?? this.env.work,
      env: this.env.childEnv(opts.env),
      timeoutMs: opts.timeoutMs,
      input: opts.input,
      transcript: this.transcript,
    });
  }

  /** Spawn a long-lived process; it is tracked so teardown can check it was stopped. */
  async spawn(cmd: string, args: string[], opts: { cwd?: string; env?: Record<string, string | undefined>; label?: string } = {}): Promise<Proc> {
    const p = await Proc.spawn(cmd, args, {
      cwd: opts.cwd ?? this.env.work,
      env: this.env.childEnv(opts.env),
      transcript: this.transcript,
      label: opts.label,
    });
    this.procs.push(p);
    return p;
  }

  /** A named, transcribed, hang-guarded step. */
  async step<T>(name: string, fn: () => Promise<T>, opts: { timeoutMs?: number } = {}): Promise<T> {
    const ms = opts.timeoutMs ?? DEFAULT_STEP_TIMEOUT_MS;
    return base.step(name, async () => {
      await this.transcript.beginStep(name);
      return withHangGuard(name, ms, fn);
    });
  }

  /** Everything this test left running. Kills it all and returns what it found. */
  async sweep(): Promise<string[]> {
    const leaks: string[] = [];
    for (const p of this.procs) {
      if (!p.stopped && !p.exited) leaks.push(`process ${p.pid} (${p.label}) still running; test never stopped it`);
      if (p.pid && !p.exited) await killTree(p.pid);
    }
    // Anything else in this test's sandbox (a daemonised BEAM, a detached node...).
    const needles = [this.env.root, `${this.env.id}_`];
    for (const p of await findProcessesMentioning(needles)) {
      if (this.procs.some((t) => t.pid === p.pid)) continue;
      leaks.push(`process ${p.pid} left behind: ${p.cmd.slice(0, 200)}`);
      await killTree(p.pid);
    }
    for (const p of this.procs) if (p.pid && isAlive(p.pid) && !p.exited) leaks.push(`process ${p.pid} survived kill`);
    // epmd registrations carrying this test's unique node-name prefix.
    const childEnv = this.env.childEnv();
    for (const n of (await epmdNames(childEnv)).names) {
      if (n.startsWith(`${this.env.id}_`)) {
        leaks.push(`epmd registration left behind: ${n}`);
        await epmdStop(n, childEnv);
      }
    }
    return leaks;
  }
}

export const test = base.extend<{ uat: Uat }, { toolchain: ToolchainInfo }>({
  toolchain: [
    async ({}, use) => {
      await use(await loadToolchain());
    },
    { scope: 'worker' },
  ],

  uat: async ({ toolchain }, use, testInfo) => {
    const env = await UatEnv.create({ bin: toolchain.bin });
    const transcript = new Transcript(join(env.root, 'transcripts'));
    const uat = new Uat(env, toolchain, transcript);
    await transcript.beginStep('setup');

    await use(uat);

    const failed = testInfo.status !== testInfo.expectedStatus;
    if (failed) {
      const dir = testInfo.outputPath('failure-bundle');
      const files = await buildFailureBundle({
        dir,
        env,
        transcript,
        toolchain,
        pids: uat.procs.map((p) => p.pid).filter((p): p is number => p !== undefined),
        title: testInfo.titlePath.join(' › '),
        repro: reproLine(toolchain, testInfo.file, testInfo.title, testInfo.project.name),
      });
      await mkdir(dir, { recursive: true });
      for (const f of files) {
        await testInfo.attach(`bundle/${f.slice(dir.length + 1).replace(/\\/g, '/')}`, { path: f });
      }
    }

    // Checked teardown: leftovers are a red step, not silently cleaned up. The sandbox
    // is removed either way (the bundle above has already copied what it needs).
    try {
      await base.step('teardown: process and epmd leak check', async () => {
        const leaks = await uat.sweep();
        expect(leaks, `left behind:\n${leaks.join('\n')}`).toEqual([]);
      });
    } finally {
      await env.dispose();
    }
  },
});

/** One line that re-runs just this test against the same toolchain. */
export function reproLine(tc: ToolchainInfo, file: string, title: string, project: string): string {
  const rel = file.replace(/\\/g, '/').replace(/^.*\/(tests\/)/, '$1');
  const sel = tc.selector; // a release tag (edge, nightly, vX.Y.Z) or local:<bin>
  return (
    `BEAMTALK_UAT_VERSION=${JSON.stringify(sel)} npm run install-toolchain && ` +
    `npx playwright test ${rel} --project=${project} -g ${JSON.stringify(title)}`
  );
}
