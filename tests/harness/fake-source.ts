// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
import { createHash } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { AssetNotFoundError, type ReleaseAsset, type ReleaseSource } from '../../lib/resolve.ts';

export const sha = (b: Buffer | string) => createHash('sha256').update(b).digest('hex');

/** An in-memory release host. `releases[tag][name]` are the bytes currently served. */
export class FakeSource implements ReleaseSource {
  downloads: string[] = [];
  digests = new Map<string, string | null>();
  constructor(
    public releases: Record<string, Record<string, Buffer>>,
    public latest = 'v1.0.0',
    public otp = { 'min-major': 28, 'max-major': 29 },
  ) {}
  async latestTag() {
    return this.latest;
  }
  async listAssets(tag: string): Promise<ReleaseAsset[]> {
    const rel = this.releases[tag];
    if (!rel) throw new Error(`no such release ${tag}`);
    return Object.entries(rel).map(([name, b]) => ({
      name,
      digest: this.digests.has(name) ? this.digests.get(name) : `sha256:${sha(b)}`,
    }));
  }
  async download(tag: string, name: string, dir: string) {
    this.downloads.push(`${tag}/${name}`);
    const b = this.releases[tag]?.[name];
    if (!b) throw new AssetNotFoundError(tag, name);
    const p = join(dir, name);
    await writeFile(p, b);
    return p;
  }
  async otpWindow() {
    return this.otp;
  }
}

export const TARGET_NAMES = ['linux-x86_64', 'macos-arm64', 'macos-x86_64', 'windows-x86_64'] as const;
export const archiveName = (t: string, v = '1.0.0') => `beamtalk-${v}-${t}.${t.startsWith('windows') ? 'zip' : 'tar.gz'}`;

export function bundleSet(version = '1.0.0', targets: readonly string[] = TARGET_NAMES): Record<string, Buffer> {
  return Object.fromEntries(targets.map((t) => [archiveName(t, version), Buffer.from(`bundle ${t} ${version}`)]));
}

export function payloadFor(
  channel: 'edge' | 'nightly' | 'release',
  assets: Record<string, Buffer>,
  tag = channel === 'release' ? 'v1.0.0' : channel,
) {
  return {
    channel,
    version: channel === 'release' ? '1.0.0' : `1.0.0-${channel}+abc1234`,
    tag,
    sha: 'abc1234def5678',
    otp: { 'min-major': 28, 'max-major': 29 },
    assets: Object.fromEntries(Object.entries(assets).map(([n, b]) => [n, sha(b)])),
  };
}
