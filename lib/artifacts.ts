// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * The failure bundle (ADR 0154 § 5): everything needed to diagnose a red without
 * re-running it. Built on every failing test and attached to the report.
 */
import { copyFile, mkdir, readdir, stat, writeFile } from 'node:fs/promises';
import { basename, join, relative } from 'node:path';
import type { ToolchainInfo } from '../schemas/index.ts';
import { epmdNames, renderProcessTree, run, type Transcript } from './proc.ts';

export interface BundleInput {
  /** Directory to write the bundle into. */
  dir: string;
  env: { root: string; home: string; work: string; id: string; childEnv(): NodeJS.ProcessEnv };
  transcript: Transcript;
  toolchain: ToolchainInfo;
  /** Pids of processes the test started (their trees go in process-tree.txt). */
  pids: number[];
  /** One-line command that re-runs just this test. */
  repro: string;
  /** Test title, for the header. */
  title: string;
}

const MAX_FILE = 5 * 1024 * 1024;
const MAX_FILES = 60;

/** Recursively find files under `root` matching `want`, skipping huge/dep dirs. */
async function findFiles(root: string, want: (rel: string) => boolean, limit = MAX_FILES): Promise<string[]> {
  const found: string[] = [];
  const walk = async (dir: string, depth: number): Promise<void> => {
    if (depth > 8 || found.length >= limit) return;
    let entries;
    try {
      entries = await readdir(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const p = join(dir, e.name);
      if (e.isDirectory()) {
        if (e.name === 'node_modules' || e.name === '.git') continue;
        await walk(p, depth + 1);
      } else if (e.isFile() && want(relative(root, p).replace(/\\/g, '/'))) {
        found.push(p);
      }
    }
  };
  await walk(root, 0);
  return found;
}

async function copyCapped(src: string, dest: string): Promise<boolean> {
  try {
    if ((await stat(src)).size > MAX_FILE) {
      await writeFile(`${dest}.skipped.txt`, `${src} is larger than ${MAX_FILE} bytes; not copied\n`);
      return false;
    }
    await mkdir(join(dest, '..'), { recursive: true });
    await copyFile(src, dest);
    return true;
  } catch {
    return false;
  }
}

/** Write the bundle; returns the files written (absolute paths). Never throws. */
export async function buildFailureBundle(b: BundleInput): Promise<string[]> {
  const written: string[] = [];
  const put = async (name: string, text: string) => {
    const p = join(b.dir, name);
    await mkdir(join(p, '..'), { recursive: true });
    await writeFile(p, text);
    written.push(p);
  };
  const safe = async (name: string, fn: () => Promise<string>) => {
    try {
      await put(name, await fn());
    } catch (e) {
      await put(name, `bundle collector failed: ${(e as Error).message}\n`).catch(() => {});
    }
  };

  await mkdir(b.dir, { recursive: true });
  await safe('repro.txt', async () => b.repro + '\n');
  await safe('summary.txt', async () =>
    [
      `test: ${b.title}`,
      `toolchain: ${b.toolchain.version} (${b.toolchain.selector})`,
      `bin: ${b.toolchain.bin}`,
      `target: ${b.toolchain.target ?? process.platform}`,
      `otp major: ${b.toolchain.otpMajor ?? '(unset)'}`,
      `env id: ${b.env.id}`,
      `root: ${b.env.root}`,
      '',
    ].join('\n'),
  );

  // Transcripts
  for (const f of await findFiles(b.transcript.dir, () => true)) {
    const dest = join(b.dir, 'transcripts', basename(f));
    if (await copyCapped(f, dest)) written.push(dest);
  }

  // Workspace + release logs, anywhere under the test's home/work.
  const logs = await findFiles(b.env.root, (rel) => /\.log$/.test(rel) && !rel.startsWith('transcripts/'));
  for (const f of logs) {
    const dest = join(b.dir, 'logs', relative(b.env.root, f));
    if (await copyCapped(f, dest)) written.push(dest);
  }

  // BEAM crash dumps (in the project dir, cwd, or the test root).
  const dumps = await findFiles(b.env.root, (rel) => basename(rel) === 'erl_crash.dump', 5);
  for (const f of dumps) {
    const dest = join(b.dir, 'crash', relative(b.env.root, f).replace(/[\\/]/g, '__'));
    if (await copyCapped(f, dest)) written.push(dest);
  }

  const childEnv = b.env.childEnv();
  await safe('epmd-names.txt', async () => (await epmdNames(childEnv)).raw);
  await safe('process-tree.txt', async () => renderProcessTree(b.pids));
  for (const [name, args] of [
    ['beamtalk-version.txt', ['--version']],
    ['beamtalk-doctor.txt', ['doctor']],
  ] as const) {
    await safe(name, async () => {
      const r = await run(b.toolchain.bin, [...args], { env: childEnv, cwd: b.env.work, timeoutMs: 45_000 });
      return `$ beamtalk ${args.join(' ')}\nexit=${r.code}\n--- stdout ---\n${r.stdout}\n--- stderr ---\n${r.stderr}\n`;
    });
  }
  return written;
}
