// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * The `resolve` step (ADR 0154 § 3, "Download once per run").
 *
 * Takes a dispatch payload (either type) or a version selector, downloads every
 * asset ONCE, checks each against its sha256 and writes them plus a manifest
 * into one directory that the workflow uploads as a single artifact. Every shard
 * then installs from that artifact and never touches the network.
 *
 * A hash that no longer matches means a rolling release (edge/nightly) was
 * replaced under us: that is `superseded` (cancelled, no failure posted), never
 * a test failure.
 */
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import {
  ArtifactPayload,
  LegacyReleasePayload,
  Manifest,
  OtpWindow,
  TargetName,
  type Channel,
} from '../schemas/index.ts';
import { sha256File } from './hash.ts';
import { parseSelector, type Selector } from './selector.ts';
import { archiveExt, loadTargets, type TargetsFile } from './targets.ts';

/** A release asset as the release API reports it. */
export interface ReleaseAsset {
  name: string;
  /** `sha256:<hex>` as reported by GitHub, when it reports one. */
  digest?: string | null;
}

/** The (network) side of resolve, injectable so the logic is testable offline. */
export interface ReleaseSource {
  /** Tag of the newest non-prerelease release. */
  latestTag(): Promise<string>;
  /** Assets currently attached to `tag`. */
  listAssets(tag: string): Promise<ReleaseAsset[]>;
  /** Download one asset into `dir`; returns its path. Throws AssetNotFoundError on 404. */
  download(tag: string, name: string, dir: string): Promise<string>;
  /** The otp-support.toml window for a ref (tag or commit). */
  otpWindow(ref: string): Promise<OtpWindow>;
}

export class AssetNotFoundError extends Error {
  constructor(
    public readonly tag: string,
    public readonly asset: string,
  ) {
    super(`asset ${asset} not found on release ${tag}`);
    this.name = 'AssetNotFoundError';
  }
}

export interface HashMismatch {
  asset: string;
  expected: string;
  actual: string;
}

/**
 * The rolling release moved on during the run. The run ends cancelled; nothing is
 * filed (ADR § 11, class `superseded`).
 */
export class SupersededError extends Error {
  constructor(
    public readonly mismatches: HashMismatch[],
    public readonly missing: string[] = [],
  ) {
    super(
      'superseded: the release changed while resolving (' +
        [
          ...mismatches.map((m) => `${m.asset}: expected ${m.expected}, got ${m.actual}`),
          ...missing.map((m) => `${m}: no longer on the release`),
        ].join('; ') +
        ')',
    );
    this.name = 'SupersededError';
  }
}

/** The release tag is final and its bytes are wrong: a real failure, not superseded. */
export class ReleaseIntegrityError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ReleaseIntegrityError';
  }
}

export type ResolveInput =
  | { kind: 'payload'; payload: ArtifactPayload }
  | { kind: 'legacy'; payload: LegacyReleasePayload }
  | { kind: 'selector'; selector: Selector };

const TOOLCHAIN_RE = /^beamtalk-(\d[^/]*)-(linux-x86_64|macos-arm64|macos-x86_64|windows-x86_64)\.(tar\.gz|zip)$/;

/** Which target a toolchain archive is for, or null (IDE archives, .vsix, ...). */
export function toolchainTarget(name: string): TargetName | null {
  const m = TOOLCHAIN_RE.exec(name);
  return m ? TargetName.parse(m[2]) : null;
}

function isRolling(channel: Channel): boolean {
  return channel !== 'release';
}

/** Map a repository_dispatch (or manual/selector) invocation to a ResolveInput. */
export function inputFromEvent(ev: {
  eventName: string;
  eventAction?: string;
  payload?: unknown;
  selector?: string;
}): ResolveInput {
  if (ev.eventName === 'repository_dispatch') {
    if (ev.eventAction === 'beamtalk-artifact') {
      return { kind: 'payload', payload: ArtifactPayload.parse(ev.payload) };
    }
    if (ev.eventAction === 'beamtalk-release') {
      return { kind: 'legacy', payload: LegacyReleasePayload.parse(ev.payload) };
    }
    throw new Error(`unsupported repository_dispatch type: ${ev.eventAction ?? '(none)'}`);
  }
  return { kind: 'selector', selector: parseSelector(ev.selector || 'latest') };
}

interface Plan {
  source: Manifest['source'];
  channel: Channel;
  version: string;
  tag: string;
  sha?: string;
  otp: OtpWindow;
  /** asset -> expected sha256 (null = nothing independent to check against) */
  expected: Map<string, string | null>;
}

/** Hash from a `.sha256` sidecar: first whitespace-separated token. */
function sidecarHash(text: string): string {
  const tok = text.trim().split(/\s+/)[0]?.toLowerCase() ?? '';
  if (!/^[0-9a-f]{64}$/.test(tok)) throw new Error(`malformed .sha256 sidecar: ${JSON.stringify(text.slice(0, 80))}`);
  return tok;
}

async function readSidecar(src: ReleaseSource, tag: string, name: string, dir: string): Promise<string | null> {
  try {
    const p = await src.download(tag, `${name}.sha256`, dir);
    return sidecarHash(await readFile(p, 'utf8'));
  } catch (e) {
    if (e instanceof AssetNotFoundError) return null;
    throw e;
  }
}

async function plan(input: ResolveInput, src: ReleaseSource, scratch: string): Promise<Plan> {
  if (input.kind === 'payload') {
    const p = input.payload;
    return {
      source: 'payload',
      channel: p.channel,
      version: p.version,
      tag: p.tag,
      sha: p.sha,
      otp: p.otp,
      expected: new Map(Object.entries(p.assets)),
    };
  }

  let tag: string;
  let channel: Channel;
  let version: string;
  let source: Plan['source'];
  if (input.kind === 'legacy') {
    tag = input.payload.tag;
    channel = 'release';
    version = input.payload.version;
    source = 'legacy';
  } else {
    const sel = input.selector;
    if (sel.kind === 'local') throw new Error('a local: selector has nothing to resolve');
    tag = sel.kind === 'latest' ? await src.latestTag() : sel.tag;
    channel = sel.kind === 'edge' ? 'edge' : sel.kind === 'nightly' ? 'nightly' : 'release';
    version = tag.replace(/^v/, '');
    source = 'selector';
  }

  // No payload `assets` map: take the toolchain archives from the release listing
  // and verify them against GitHub's digest, else the published `.sha256` sidecar.
  const listed = await src.listAssets(tag);
  const expected = new Map<string, string | null>();
  for (const a of listed) {
    if (toolchainTarget(a.name) === null) continue;
    const digest = a.digest?.startsWith('sha256:') ? a.digest.slice(7).toLowerCase() : null;
    if (digest && /^[0-9a-f]{64}$/.test(digest)) {
      expected.set(a.name, digest);
    } else {
      expected.set(a.name, await readSidecar(src, tag, a.name, scratch));
    }
  }
  if (expected.size === 0) throw new Error(`release ${tag} has no toolchain archives`);

  const otp = await src.otpWindow(channel === 'release' ? tag : 'main');
  return { source, channel, version, tag, otp, expected };
}

/**
 * Resolve `input` into `outDir` (assets/ + manifest.json). Throws
 * {@link SupersededError} when any hash no longer matches or an asset vanished
 * from a rolling release.
 */
export async function resolve(input: ResolveInput, src: ReleaseSource, outDir: string): Promise<Manifest> {
  const assetsDir = join(outDir, 'assets');
  const scratch = join(outDir, '.scratch');
  await rm(outDir, { recursive: true, force: true });
  await mkdir(assetsDir, { recursive: true });
  await mkdir(scratch, { recursive: true });

  const pl = await plan(input, src, scratch);

  const mismatches: HashMismatch[] = [];
  const missing: string[] = [];
  const assets: Manifest['assets'] = {};
  const unverified: string[] = [];

  for (const [name, expected] of pl.expected) {
    let path: string;
    try {
      path = await src.download(pl.tag, name, assetsDir);
    } catch (e) {
      if (e instanceof AssetNotFoundError) {
        if (isRolling(pl.channel)) {
          missing.push(name);
          continue;
        }
        throw new ReleaseIntegrityError(`release ${pl.tag} lists asset ${name} but it is missing`);
      }
      throw e;
    }
    const { sha256, size } = await sha256File(path);
    if (expected === null) {
      unverified.push(name);
    } else if (sha256 !== expected) {
      mismatches.push({ asset: name, expected, actual: sha256 });
      continue;
    }
    assets[name] = { sha256, size };
  }

  if (mismatches.length || missing.length) {
    // Rolling releases are replaced in place: that is a moved target, not a bug.
    // A tagged release is immutable: a mismatch there is a real integrity failure.
    if (isRolling(pl.channel)) throw new SupersededError(mismatches, missing);
    throw new ReleaseIntegrityError(
      'published release does not match its recorded sha256: ' +
        mismatches.map((m) => `${m.asset} expected ${m.expected} got ${m.actual}`).join('; '),
    );
  }

  const toolchains: Manifest['toolchains'] = {};
  for (const name of Object.keys(assets)) {
    const t = toolchainTarget(name);
    if (t) toolchains[t] = name;
  }

  await rm(scratch, { recursive: true, force: true });
  const manifest = Manifest.parse({
    schemaVersion: 1,
    source: pl.source,
    channel: pl.channel,
    version: pl.version,
    tag: pl.tag,
    sha: pl.sha,
    otp: pl.otp,
    assets,
    toolchains,
    unverified,
  });
  await writeFile(join(outDir, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
  return manifest;
}

export interface MatrixLeg {
  target: TargetName;
  runner: string;
  otp: number;
  otp_role: 'min' | 'max';
  grep: string;
  shard: number;
  shards: number;
  name: string;
}

/**
 * The (target × OTP × shard) legs to run for `manifest`, per targets.toml. A
 * target whose archive is absent is skipped where it is optional for the channel
 * and an error where it is required.
 */
export function buildMatrix(manifest: Manifest, targets: TargetsFile = loadTargets()): MatrixLeg[] {
  for (const [name, def] of Object.entries(targets.targets)) {
    const t = TargetName.parse(name);
    if (def && def.required.includes(manifest.channel) && !manifest.toolchains[t]) {
      throw new Error(
        `channel ${manifest.channel} requires a ${t} toolchain archive (beamtalk-*-${t}.${archiveExt(t)}) but ${manifest.tag} has none`,
      );
    }
  }
  const legs: MatrixLeg[] = [];
  for (const leg of targets.leg) {
    if (!leg.channels.includes(manifest.channel)) continue;
    if (!manifest.toolchains[leg.target]) continue;
    const def = targets.targets[leg.target];
    if (!def) throw new Error(`targets.toml: leg for undeclared target ${leg.target}`);
    const otp = leg.otp === 'min' ? manifest.otp['min-major'] : manifest.otp['max-major'];
    for (let shard = 1; shard <= leg.shards; shard++) {
      legs.push({
        target: leg.target,
        runner: def.runner,
        otp,
        otp_role: leg.otp,
        grep: leg.grep ?? '',
        shard,
        shards: leg.shards,
        name: `${leg.target} · OTP ${otp}` + (leg.shards > 1 ? ` · ${shard}/${leg.shards}` : ''),
      });
    }
  }
  return legs;
}
