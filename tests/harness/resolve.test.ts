// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
//
// Tests of the resolve step. Each guards a specific failure: a tampered or
// vanished asset must end the run `superseded`, never green and never red.

import { expect, test } from '@playwright/test';
import { mkdtemp, readFile, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  ReleaseIntegrityError,
  SupersededError,
  buildMatrix,
  inputFromEvent,
  resolve,
} from '../../lib/resolve.ts';
import { loadTargets } from '../../lib/targets.ts';
import { ArtifactPayload } from '../../schemas/index.ts';
import { FakeSource, TARGET_NAMES, archiveName, bundleSet, payloadFor, sha } from './fake-source.ts';

const out = () => mkdtemp(join(tmpdir(), 'btuat-resolve-'));

test.describe('beamtalk-artifact payload', () => {
  test('downloads each asset once and writes a verified manifest', async () => {
    const assets = { ...bundleSet(), 'beamtalk-ide-1.0.0-linux-x86_64.tar.gz': Buffer.from('ide'), 'x.vsix': Buffer.from('vsix') };
    const src = new FakeSource({ nightly: assets });
    const dir = await out();
    const m = await resolve({ kind: 'payload', payload: ArtifactPayload.parse(payloadFor('nightly', assets)) }, src, dir);
    expect(src.downloads.sort()).toEqual(Object.keys(assets).map((n) => `nightly/${n}`).sort()); // exactly once each
    expect(Object.keys(m.toolchains).sort()).toEqual([...TARGET_NAMES].sort());
    expect(m.toolchains['linux-x86_64']).toBe(archiveName('linux-x86_64')); // IDE archive is not a toolchain
    expect(await readdir(join(dir, 'assets'))).toHaveLength(Object.keys(assets).length);
    expect(JSON.parse(await readFile(join(dir, 'manifest.json'), 'utf8')).tag).toBe('nightly');
  });

  test('a sha256 mismatch ends the run superseded (rolling release replaced under us)', async () => {
    const assets = bundleSet();
    const payload = ArtifactPayload.parse(payloadFor('edge', assets));
    // The edge release is republished after the dispatch was sent: new bytes, same name.
    const replaced = { ...assets, [archiveName('linux-x86_64')]: Buffer.from('a newer build') };
    const src = new FakeSource({ edge: replaced });
    const err = await resolve({ kind: 'payload', payload }, src, await out()).catch((e) => e);
    expect(err).toBeInstanceOf(SupersededError);
    expect((err as SupersededError).mismatches).toEqual([
      { asset: archiveName('linux-x86_64'), expected: sha(assets[archiveName('linux-x86_64')]!), actual: sha('a newer build') },
    ]);
  });

  test('an asset that vanished from a rolling release is superseded too', async () => {
    const assets = bundleSet();
    const payload = ArtifactPayload.parse(payloadFor('nightly', assets));
    const { [archiveName('macos-arm64')]: _gone, ...rest } = assets;
    const err = await resolve({ kind: 'payload', payload }, new FakeSource({ nightly: rest }), await out()).catch((e) => e);
    expect(err).toBeInstanceOf(SupersededError);
    expect((err as SupersededError).missing).toEqual([archiveName('macos-arm64')]);
  });

  test('a mismatch on a tagged release is a real integrity failure, not superseded', async () => {
    const assets = bundleSet();
    const payload = ArtifactPayload.parse(payloadFor('release', assets));
    const tampered = { ...assets, [archiveName('windows-x86_64')]: Buffer.from('evil') };
    const err = await resolve({ kind: 'payload', payload }, new FakeSource({ 'v1.0.0': tampered }), await out()).catch((e) => e);
    expect(err).toBeInstanceOf(ReleaseIntegrityError);
    expect(err).not.toBeInstanceOf(SupersededError);
  });

  test('the payload schema rejects malformed payloads', () => {
    const good = payloadFor('edge', bundleSet());
    expect(() => ArtifactPayload.parse(good)).not.toThrow();
    const bad: Record<string, unknown>[] = [
      { ...good, channel: 'beta' },
      { ...good, assets: {} },
      { ...good, assets: { 'a.tar.gz': 'not-a-hash' } },
      { ...good, assets: { '../escape.tar.gz': 'a'.repeat(64) } },
      { ...good, assets: { 'sub/dir.tar.gz': 'a'.repeat(64) } },
      { ...good, otp: { 'min-major': 29, 'max-major': 28 } },
      { ...good, sha: 'zzz' },
    ];
    for (const b of bad) expect(() => ArtifactPayload.parse(b), JSON.stringify(b).slice(0, 80)).toThrow();
  });
});

test.describe('legacy beamtalk-release payload', () => {
  test('verifies against the published .sha256 files', async () => {
    const assets = bundleSet();
    const rel: Record<string, Buffer> = { ...assets };
    for (const [n, b] of Object.entries(assets)) rel[`${n}.sha256`] = Buffer.from(`${sha(b)}  ${n}\n`);
    const src = new FakeSource({ 'v1.0.0': rel });
    src.digests = new Map(Object.keys(assets).map((n) => [n, null])); // force the sidecar path
    const m = await resolve({ kind: 'legacy', payload: { version: '1.0.0', tag: 'v1.0.0' } }, src, await out());
    expect(m.source).toBe('legacy');
    expect(m.channel).toBe('release');
    expect(m.unverified).toEqual([]);
    expect(src.downloads.filter((d) => d.endsWith('.sha256'))).toHaveLength(4);
  });

  test('a bundle that disagrees with its .sha256 sidecar fails (never installs)', async () => {
    const assets = bundleSet();
    const rel: Record<string, Buffer> = { ...assets };
    for (const [n, b] of Object.entries(assets)) rel[`${n}.sha256`] = Buffer.from(`${sha(b)}  ${n}\n`);
    rel[archiveName('macos-x86_64')] = Buffer.from('corrupted after the sidecar was written');
    const src = new FakeSource({ 'v1.0.0': rel });
    src.digests = new Map(Object.keys(assets).map((n) => [n, null]));
    const err = await resolve({ kind: 'legacy', payload: { version: '1.0.0', tag: 'v1.0.0' } }, src, await out()).catch((e) => e);
    expect(err).toBeInstanceOf(ReleaseIntegrityError);
    expect(String(err)).toContain(archiveName('macos-x86_64'));
  });

  test('both event types map to inputs; unknown types are rejected', () => {
    const art = inputFromEvent({ eventName: 'repository_dispatch', eventAction: 'beamtalk-artifact', payload: payloadFor('edge', bundleSet()) });
    expect(art.kind).toBe('payload');
    const leg = inputFromEvent({ eventName: 'repository_dispatch', eventAction: 'beamtalk-release', payload: { version: '1.0.0', tag: 'v1.0.0' } });
    expect(leg.kind).toBe('legacy');
    expect(() => inputFromEvent({ eventName: 'repository_dispatch', eventAction: 'something-else', payload: {} })).toThrow(/unsupported/);
    expect(() => inputFromEvent({ eventName: 'repository_dispatch', eventAction: 'beamtalk-release', payload: { version: '1.0.0', tag: 'nightly' } })).toThrow();
  });
});

test.describe('selector path', () => {
  test('latest resolves the newest tag and verifies against GitHub digests', async () => {
    const src = new FakeSource({ 'v1.2.3': bundleSet('1.2.3') }, 'v1.2.3');
    const m = await resolve(inputFromEvent({ eventName: 'workflow_dispatch', selector: 'latest' }), src, await out());
    expect(m.tag).toBe('v1.2.3');
    expect(m.channel).toBe('release');
    expect(m.otp).toEqual({ 'min-major': 28, 'max-major': 29 });
  });

  test('an asset with no digest and no sidecar is flagged unverified, not trusted silently', async () => {
    const src = new FakeSource({ edge: bundleSet('1.0.0', ['linux-x86_64']) });
    src.digests = new Map([[archiveName('linux-x86_64'), null]]);
    const m = await resolve(inputFromEvent({ eventName: 'workflow_dispatch', selector: 'edge' }), src, await out());
    expect(m.unverified).toEqual([archiveName('linux-x86_64')]);
  });
});

test.describe('matrix', () => {
  const manifestFor = async (channel: 'edge' | 'nightly' | 'release', targets: readonly string[], otp = { 'min-major': 28, 'max-major': 29 }) => {
    const assets = bundleSet('1.0.0', targets);
    const payload = ArtifactPayload.parse({ ...payloadFor(channel, assets), otp });
    return resolve({ kind: 'payload', payload }, new FakeSource({ [payload.tag]: assets }), await out());
  };

  test('nightly: the per-target OTP assignment of ADR § 3', async () => {
    const legs = buildMatrix(await manifestFor('nightly', TARGET_NAMES), loadTargets());
    const key = legs.map((l) => `${l.target}@${l.otp}${l.grep ? ` ${l.grep}` : ''}`).sort();
    expect(key).toEqual(
      [
        'linux-x86_64@28',
        'linux-x86_64@29 @max-otp',
        'macos-arm64@29',
        'macos-x86_64@28',
        'windows-x86_64@29',
      ].sort(),
    );
    expect(legs.find((l) => l.target === 'macos-arm64')?.runner).toBe('macos-15');
    expect(legs.find((l) => l.target === 'macos-x86_64')?.runner).toBe('macos-15-intel');
    expect(legs.find((l) => l.target === 'windows-x86_64')?.runner).toBe('windows-2022');
    expect(legs.find((l) => l.target === 'linux-x86_64')?.runner).toBe('ubuntu-24.04');
  });

  test('an OTP window bump in the payload needs no uat edit', async () => {
    const legs = buildMatrix(await manifestFor('release', TARGET_NAMES, { 'min-major': 29, 'max-major': 30 }), loadTargets());
    expect(legs.filter((l) => l.otp_role === 'min').every((l) => l.otp === 29)).toBe(true);
    expect(legs.filter((l) => l.otp_role === 'max').every((l) => l.otp === 30)).toBe(true);
  });

  test('edge with only a linux asset runs just the linux min leg', async () => {
    const legs = buildMatrix(await manifestFor('edge', ['linux-x86_64']), loadTargets());
    expect(legs.map((l) => `${l.target}@${l.otp}`)).toEqual(['linux-x86_64@28']);
  });

  test('edge picks up macos-arm64 as soon as the asset exists', async () => {
    const legs = buildMatrix(await manifestFor('edge', ['linux-x86_64', 'macos-arm64']), loadTargets());
    expect(legs.map((l) => `${l.target}@${l.otp}`).sort()).toEqual(['linux-x86_64@28', 'macos-arm64@29']);
  });

  test('a required target with no archive is an error, not a silently smaller matrix', async () => {
    const m = await manifestFor('release', ['linux-x86_64', 'macos-arm64', 'macos-x86_64']);
    expect(() => buildMatrix(m, loadTargets())).toThrow(/requires a windows-x86_64/);
  });
});
