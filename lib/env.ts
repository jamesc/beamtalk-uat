// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * An isolated environment for one test (ADR 0154 § 9): its own HOME, port
 * allocator, unique Erlang node names and cookies. Nothing from the runner's own
 * user state (config, credentials, tokens) reaches the toolchain under test.
 */
import { randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, realpath, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { delimiter, dirname, join } from 'node:path';

/** Variables that must not leak into the toolchain under test. */
const STRIP = [
  /^BEAMTALK_/i,
  /^RELEASE_/,
  /^ERL_/,
  /^REBAR/,
  /^HEX_/,
  /^GH_TOKEN$/,
  /^GITHUB_TOKEN$/,
  /^ACTIONS_/,
  /^NODE_OPTIONS$/,
  /^npm_/i,
];

const usedPorts = new Set<number>();

/** Allocate a free TCP port on loopback, never handing the same port out twice. */
export async function allocatePort(): Promise<number> {
  for (let i = 0; i < 50; i++) {
    const port = await new Promise<number>((resolve, reject) => {
      const srv = createServer();
      srv.once('error', reject);
      srv.listen(0, '127.0.0.1', () => {
        const p = (srv.address() as { port: number }).port;
        srv.close(() => resolve(p));
      });
    });
    if (!usedPorts.has(port)) {
      usedPorts.add(port);
      return port;
    }
  }
  throw new Error('could not allocate a unique free port');
}

export class UatEnv {
  /** Short unique id; prefixes node names so epmd leaks are attributable. */
  readonly id: string;
  /** Everything for this test lives under here. */
  readonly root: string;
  readonly home: string;
  readonly work: string;
  readonly tmp: string;
  private seq = 0;
  private readonly extraPath: string[];

  private constructor(root: string, id: string, extraPath: string[]) {
    this.root = root;
    this.id = id;
    this.home = join(root, 'home');
    this.work = join(root, 'work');
    this.tmp = join(root, 'tmp');
    this.extraPath = extraPath;
  }

  static async create(opts: { bin?: string; extraPath?: string[] } = {}): Promise<UatEnv> {
    // realpath: macOS tmp is a symlink, and leak scans match on the real path.
    const root = await realpath(await mkdtemp(join(tmpdir(), 'btuat-')));
    const id = 'u' + randomBytes(3).toString('hex');
    const path = [...(opts.bin ? [dirname(opts.bin)] : []), ...(opts.extraPath ?? [])];
    const env = new UatEnv(root, id, path);
    for (const d of [env.home, env.work, env.tmp]) await mkdir(d, { recursive: true });
    return env;
  }

  /** A unique Erlang short node name, e.g. `u1a2b3c_1`. Valid for -sname. */
  nodeName(label = 'n'): string {
    const clean = label.toLowerCase().replace(/[^a-z0-9]/g, '');
    return `${this.id}_${clean}${++this.seq}`;
  }

  /** A fresh random Erlang cookie. */
  cookie(): string {
    return randomBytes(12).toString('hex').toUpperCase();
  }

  port(): Promise<number> {
    return allocatePort();
  }

  /** The environment handed to every child process. */
  childEnv(extra: Record<string, string | undefined> = {}): NodeJS.ProcessEnv {
    const out: NodeJS.ProcessEnv = {};
    for (const [k, v] of Object.entries(process.env)) {
      if (v === undefined || STRIP.some((re) => re.test(k))) continue;
      out[k] = v;
    }
    const pathKey = Object.keys(out).find((k) => k.toUpperCase() === 'PATH') ?? 'PATH';
    out[pathKey] = [...this.extraPath, out[pathKey] ?? ''].filter(Boolean).join(delimiter);
    Object.assign(out, {
      HOME: this.home,
      USERPROFILE: this.home,
      APPDATA: join(this.home, 'AppData', 'Roaming'),
      LOCALAPPDATA: join(this.home, 'AppData', 'Local'),
      XDG_CONFIG_HOME: join(this.home, '.config'),
      XDG_CACHE_HOME: join(this.home, '.cache'),
      XDG_DATA_HOME: join(this.home, '.local', 'share'),
      TMPDIR: this.tmp,
      TEMP: this.tmp,
      TMP: this.tmp,
      NO_COLOR: '1',
    });
    for (const [k, v] of Object.entries(extra)) {
      if (v === undefined) delete out[k];
      else out[k] = v;
    }
    return out;
  }

  async dispose(): Promise<void> {
    await rm(this.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  }
}
