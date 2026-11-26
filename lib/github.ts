// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/** The real ReleaseSource: GitHub releases of jamesc/beamtalk via the `gh` CLI (REST only). */
import { execFile } from 'node:child_process';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { parse } from 'smol-toml';
import { OtpWindow } from '../schemas/index.ts';
import { AssetNotFoundError, ReleaseNotFoundError, type ReleaseAsset, type ReleaseSource } from './resolve.ts';

const exec = promisify(execFile);
export const RELEASE_REPO = process.env.BEAMTALK_UAT_REPO || 'jamesc/beamtalk';

async function gh(args: string[]): Promise<string> {
  try {
    const { stdout } = await exec('gh', args, { maxBuffer: 64 * 1024 * 1024 });
    return stdout;
  } catch (e) {
    const err = e as { stderr?: string; message: string };
    const text = `${err.stderr ?? ''}${err.message}`;
    if (/release not found|HTTP 404|no assets match/i.test(text)) {
      throw Object.assign(new Error(text), { notFound: true });
    }
    throw new Error(`gh ${args.join(' ')} failed: ${text}`);
  }
}

export const githubSource: ReleaseSource = {
  async latestTag() {
    try {
      const out = await gh(['api', `repos/${RELEASE_REPO}/releases/latest`, '--jq', '.tag_name']);
      return out.trim();
    } catch (e) {
      if ((e as { notFound?: boolean }).notFound) throw new ReleaseNotFoundError('latest');
      throw e;
    }
  },

  async listAssets(tag): Promise<ReleaseAsset[]> {
    try {
      const out = await gh([
        'api',
        `repos/${RELEASE_REPO}/releases/tags/${tag}`,
        '--jq',
        '[.assets[] | {name, digest}]',
      ]);
      return JSON.parse(out) as ReleaseAsset[];
    } catch (e) {
      if ((e as { notFound?: boolean }).notFound) throw new ReleaseNotFoundError(tag);
      throw e;
    }
  },

  async download(tag, name, dir) {
    try {
      await gh(['release', 'download', tag, '-R', RELEASE_REPO, '--pattern', name, '--dir', dir, '--clobber']);
    } catch (e) {
      if ((e as { notFound?: boolean }).notFound) throw new AssetNotFoundError(tag, name);
      throw e;
    }
    return join(dir, name);
  },

  async otpWindow(ref) {
    const out = await gh([
      'api',
      '-H',
      'Accept: application/vnd.github.raw+json',
      `repos/${RELEASE_REPO}/contents/otp-support.toml?ref=${encodeURIComponent(ref)}`,
    ]);
    const t = parse(out) as Record<string, unknown>;
    return OtpWindow.parse({ 'min-major': t['min-major'], 'max-major': t['max-major'] });
  },
};
