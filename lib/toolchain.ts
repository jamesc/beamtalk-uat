// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * Installing the toolchain under test.
 *
 * Shards install from the `resolve` job's artifact (manifest + assets); local
 * development resolves the selector in-process. Either way the thing installed is
 * the released bundle, extracted the way a user would, never built from source.
 */
import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, readFile, readdir, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, isAbsolute, join, resolve as resolvePath } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { Manifest, ToolchainInfo, type TargetName } from '../schemas/index.ts';
import { githubSource } from './github.ts';
import { sha256File } from './hash.ts';
import { inputFromEvent, resolve, type ReleaseSource } from './resolve.ts';
import { describeSelector, parseSelector } from './selector.ts';
import { hostTarget } from './targets.ts';

const exec = promisify(execFile);

export const REPO_ROOT = resolvePath(dirname(fileURLToPath(import.meta.url)), '..');
/** Scratch state for this run; overridable so the harness can test itself in a nested run. */
export const uatDir = (): string => process.env.BEAMTALK_UAT_DIR || join(REPO_ROOT, '.uat');
export const toolchainJson = (): string => join(uatDir(), 'toolchain.json');

export function binName(target: TargetName = hostTarget()): string {
  return target === 'windows-x86_64' ? 'beamtalk.exe' : 'beamtalk';
}

async function extract(archive: string, dest: string): Promise<void> {
  await mkdir(dest, { recursive: true });
  // bsdtar (macOS, Windows) and GNU tar both read tar.gz; bsdtar also reads zip.
  await exec('tar', ['-xf', archive, '-C', dest]);
}

/** Run `<bin> --version` and return the trimmed first line. */
export async function queryVersion(bin: string): Promise<string> {
  const { stdout } = await exec(bin, ['--version'], { timeout: 30_000 });
  const v = stdout.trim().split(/\r?\n/)[0] ?? '';
  if (!v) throw new Error(`\`${bin} --version\` printed nothing`);
  return v;
}

/**
 * Install `target`'s toolchain from a resolve artifact directory into `prefix`.
 * Re-checks the archive's sha256 against the manifest: a shard must never test
 * bytes the resolve job did not verify.
 */
export async function installFromManifest(
  artifactDir: string,
  target: TargetName,
  prefix: string,
): Promise<ToolchainInfo> {
  const manifest = Manifest.parse(JSON.parse(await readFile(join(artifactDir, 'manifest.json'), 'utf8')));
  const name = manifest.toolchains[target];
  if (!name) throw new Error(`manifest has no toolchain archive for ${target}`);
  const archive = join(artifactDir, 'assets', name);
  const want = manifest.assets[name]!.sha256;
  const got = (await sha256File(archive)).sha256;
  if (got !== want) {
    throw new Error(`artifact corrupted: ${name} sha256 ${got} != manifest ${want}`);
  }

  const staging = `${prefix}.staging`;
  await rm(staging, { recursive: true, force: true });
  await extract(archive, staging);
  // Archives carry one top-level `beamtalk-<version>/` directory.
  const entries = await readdir(staging, { withFileTypes: true });
  const dirs = entries.filter((e) => e.isDirectory());
  if (dirs.length !== 1 || entries.length !== 1) {
    throw new Error(`unexpected archive layout in ${name}: ${entries.map((e) => e.name).join(', ')}`);
  }
  await rm(prefix, { recursive: true, force: true });
  await rename(join(staging, dirs[0]!.name), prefix);
  await rm(staging, { recursive: true, force: true });

  const bin = join(prefix, 'bin', binName(target));
  if (!existsSync(bin)) throw new Error(`install did not produce ${bin}: archive layout changed?`);
  return {
    bin,
    version: await queryVersion(bin),
    prefix,
    selector: `${manifest.channel}:${manifest.tag}`,
    target,
  };
}

/** Use an already-installed binary. */
export async function useLocal(path: string): Promise<ToolchainInfo> {
  const bin = isAbsolute(path) ? path : resolvePath(path);
  if (!existsSync(bin)) throw new Error(`local toolchain does not exist: ${bin}`);
  return { bin, version: await queryVersion(bin), selector: `local:${bin}` };
}

export interface PrepareOptions {
  /** Selector string; default: BEAMTALK_UAT_VERSION or `latest`. */
  selector?: string;
  /** A resolve artifact directory (CI shards); overrides the selector. */
  artifactDir?: string;
  target?: TargetName;
  source?: ReleaseSource;
  /** Where to install; default `.uat/install`. */
  prefix?: string;
}

/**
 * Produce the ToolchainInfo for this run and persist it to `.uat/toolchain.json`
 * for the test workers.
 */
export async function prepareToolchain(opts: PrepareOptions = {}): Promise<ToolchainInfo> {
  const target = opts.target ?? hostTarget();
  const prefix = opts.prefix ?? join(uatDir(), 'install');
  const artifactDir = opts.artifactDir ?? process.env.BEAMTALK_UAT_ARTIFACT_DIR;
  let info: ToolchainInfo;

  if (artifactDir) {
    info = await installFromManifest(artifactDir, target, prefix);
  } else {
    const selector = parseSelector(opts.selector ?? process.env.BEAMTALK_UAT_VERSION ?? 'latest');
    if (selector.kind === 'local') {
      info = await useLocal(selector.path);
    } else {
      // Local development: do what the resolve job does, in-process.
      const out = join(uatDir(), 'resolved');
      await resolve(inputFromEvent({ eventName: 'workflow_dispatch', selector: describeSelector(selector) }), opts.source ?? githubSource, out);
      info = await installFromManifest(out, target, prefix);
    }
  }
  const otpMajor = process.env.BEAMTALK_UAT_OTP ? Number(process.env.BEAMTALK_UAT_OTP) : undefined;
  info = ToolchainInfo.parse({ ...info, otpMajor });
  await mkdir(uatDir(), { recursive: true });
  await writeFile(toolchainJson(), JSON.stringify(info, null, 2) + '\n');
  return info;
}

export async function loadToolchain(): Promise<ToolchainInfo> {
  if (!existsSync(toolchainJson())) {
    throw new Error('no toolchain prepared: run through `playwright test` (globalSetup) or `npm run uat`');
  }
  return ToolchainInfo.parse(JSON.parse(await readFile(toolchainJson(), 'utf8')));
}
