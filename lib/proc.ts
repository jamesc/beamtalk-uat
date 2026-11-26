// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * Process control: spawn, readiness, transcripts, process-tree kill, leak scans.
 */
import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { createConnection } from 'node:net';
import { mkdir, appendFile } from 'node:fs/promises';
import { join } from 'node:path';
import { promisify } from 'node:util';

const execP = promisify(execFile);
const IS_WIN = process.platform === 'win32';

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Per-step transcript files: every command, its output and exit status. */
export class Transcript {
  private n = 0;
  private current: string | null = null;
  constructor(readonly dir: string) {}

  /** Start a new step file; returns its path. */
  async beginStep(name: string): Promise<string> {
    await mkdir(this.dir, { recursive: true });
    const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 60) || 'step';
    this.current = join(this.dir, `${String(++this.n).padStart(2, '0')}-${slug}.log`);
    await appendFile(this.current, `# step ${this.n}: ${name}\n# started ${new Date().toISOString()}\n`);
    return this.current;
  }

  async write(text: string): Promise<void> {
    if (!this.current) await this.beginStep('setup');
    await appendFile(this.current!, text);
  }
}

export interface RunOptions {
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  input?: string;
  /** Hard limit; the whole process tree is killed when it is hit. Default 60s. */
  timeoutMs?: number;
  transcript?: Transcript;
}

export interface RunResult {
  code: number | null;
  signal: NodeJS.Signals | null;
  stdout: string;
  stderr: string;
  durationMs: number;
  timedOut: boolean;
}

function render(cmd: string, args: string[]): string {
  return [cmd, ...args].map((a) => (/[\s"']/.test(a) ? JSON.stringify(a) : a)).join(' ');
}

/** Run a command to completion, recording it in the transcript. Never throws on a non-zero exit. */
export async function run(cmd: string, args: string[], opts: RunOptions = {}): Promise<RunResult> {
  const timeoutMs = opts.timeoutMs ?? 60_000;
  const started = Date.now();
  await opts.transcript?.write(`\n$ ${render(cmd, args)}   (cwd=${opts.cwd ?? process.cwd()})\n`);
  const child = spawn(cmd, args, {
    cwd: opts.cwd,
    env: opts.env,
    detached: !IS_WIN,
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  });
  let stdout = '';
  let stderr = '';
  child.stdout!.on('data', (d: Buffer) => (stdout += d.toString()));
  child.stderr!.on('data', (d: Buffer) => (stderr += d.toString()));
  child.stdin!.on('error', () => {});
  child.stdin!.end(opts.input ?? '');

  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    if (child.pid) void killTree(child.pid);
  }, timeoutMs);

  const { code, signal } = await new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve) => {
    child.once('error', (e) => {
      stderr += `spawn error: ${e.message}\n`;
      resolve({ code: 127, signal: null });
    });
    child.once('close', (code, signal) => resolve({ code, signal }));
  });
  clearTimeout(timer);
  const res: RunResult = { code, signal, stdout, stderr, durationMs: Date.now() - started, timedOut };
  await opts.transcript?.write(
    `${stdout ? `[stdout]\n${stdout.endsWith('\n') ? stdout : stdout + '\n'}` : ''}` +
      `${stderr ? `[stderr]\n${stderr.endsWith('\n') ? stderr : stderr + '\n'}` : ''}` +
      `[exit] code=${code} signal=${signal} timedOut=${timedOut} ${res.durationMs}ms\n`,
  );
  return res;
}

/** A long-lived child process with readiness helpers. Obtain via {@link Proc.spawn}. */
export class Proc {
  stdout = '';
  stderr = '';
  exitCode: number | null = null;
  exited = false;
  /** True once the test stopped it deliberately (so teardown doesn't call it a leak). */
  stopped = false;
  readonly done: Promise<void>;

  private constructor(
    readonly child: ChildProcess,
    readonly label: string,
    private readonly transcript?: Transcript,
  ) {
    child.stdout!.on('data', (d: Buffer) => {
      const s = d.toString();
      this.stdout += s;
      void transcript?.write(`[${label} stdout] ${s}`);
    });
    child.stderr!.on('data', (d: Buffer) => {
      const s = d.toString();
      this.stderr += s;
      void transcript?.write(`[${label} stderr] ${s}`);
    });
    this.done = new Promise<void>((resolve) => {
      child.once('close', (code) => {
        this.exited = true;
        this.exitCode = code;
        void transcript?.write(`[${label} exit] code=${code}\n`);
        resolve();
      });
      child.once('error', () => {
        this.exited = true;
        resolve();
      });
    });
  }

  static async spawn(
    cmd: string,
    args: string[],
    opts: { cwd?: string; env?: NodeJS.ProcessEnv; transcript?: Transcript; label?: string } = {},
  ): Promise<Proc> {
    const label = opts.label ?? cmd;
    await opts.transcript?.write(`\n$ ${render(cmd, args)} &   (cwd=${opts.cwd ?? process.cwd()})\n`);
    const child = spawn(cmd, args, {
      cwd: opts.cwd,
      env: opts.env,
      detached: !IS_WIN,
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
    });
    child.stdin!.on('error', () => {});
    return new Proc(child, label, opts.transcript);
  }

  get pid(): number | undefined {
    return this.child.pid;
  }

  write(text: string): void {
    this.child.stdin!.write(text);
  }

  /** Resolve when `re` appears in stdout+stderr; reject on timeout or early exit. */
  async waitForOutput(re: RegExp, timeoutMs = 30_000): Promise<RegExpExecArray> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const m = re.exec(this.stdout + this.stderr);
      if (m) return m;
      if (this.exited) {
        throw new Error(`${this.label} exited (code ${this.exitCode}) before output matched ${re}\n${this.tail()}`);
      }
      if (Date.now() > deadline) {
        throw new Error(`${this.label}: no output matching ${re} within ${timeoutMs}ms\n${this.tail()}`);
      }
      await sleep(50);
    }
  }

  /** Resolve when something accepts TCP connections on host:port. */
  async waitForPort(port: number, host = '127.0.0.1', timeoutMs = 30_000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      if (await canConnect(port, host)) return;
      if (this.exited) throw new Error(`${this.label} exited (code ${this.exitCode}) before ${host}:${port} opened\n${this.tail()}`);
      if (Date.now() > deadline) throw new Error(`${this.label}: ${host}:${port} not open within ${timeoutMs}ms\n${this.tail()}`);
      await sleep(100);
    }
  }

  private tail(): string {
    const t = (this.stdout + this.stderr).split('\n').slice(-20).join('\n');
    return `--- last output of ${this.label} ---\n${t}`;
  }

  /** Stop the process and everything it started. */
  async stop(): Promise<void> {
    this.stopped = true;
    if (this.exited) return;
    if (this.child.pid) await killTree(this.child.pid);
    await Promise.race([this.done, sleep(5_000)]);
  }
}

export function canConnect(port: number, host: string): Promise<boolean> {
  return new Promise((resolve) => {
    const s = createConnection({ port, host });
    s.once('connect', () => (s.destroy(), resolve(true)));
    s.once('error', () => (s.destroy(), resolve(false)));
  });
}

export interface ProcInfo {
  pid: number;
  ppid: number;
  cmd: string;
}

/** Snapshot of every process on the machine. */
export async function listProcesses(): Promise<ProcInfo[]> {
  if (IS_WIN) {
    const { stdout } = await execP(
      'powershell',
      [
        '-NoProfile',
        '-Command',
        'Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,CommandLine | ConvertTo-Json -Compress',
      ],
      { maxBuffer: 64 * 1024 * 1024 },
    );
    const raw = JSON.parse(stdout || '[]') as
      | { ProcessId: number; ParentProcessId: number; CommandLine: string | null }
      | { ProcessId: number; ParentProcessId: number; CommandLine: string | null }[];
    return (Array.isArray(raw) ? raw : [raw]).map((p) => ({
      pid: p.ProcessId,
      ppid: p.ParentProcessId,
      cmd: p.CommandLine ?? '',
    }));
  }
  const { stdout } = await execP('ps', ['-axo', 'pid=,ppid=,args='], { maxBuffer: 64 * 1024 * 1024 });
  return stdout
    .split('\n')
    .map((l) => /^\s*(\d+)\s+(\d+)\s+(.*)$/.exec(l))
    .filter((m): m is RegExpExecArray => m !== null)
    .map((m) => ({ pid: Number(m[1]), ppid: Number(m[2]), cmd: m[3]! }));
}

/** All descendants of `root` (not including it) in `procs`. */
export function descendantsOf(root: number, procs: ProcInfo[]): ProcInfo[] {
  const kids = new Map<number, ProcInfo[]>();
  for (const p of procs) kids.set(p.ppid, [...(kids.get(p.ppid) ?? []), p]);
  const out: ProcInfo[] = [];
  const walk = (pid: number) => {
    for (const k of kids.get(pid) ?? []) {
      out.push(k);
      walk(k.pid);
    }
  };
  walk(root);
  return out;
}

export function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as NodeJS.ErrnoException).code === 'EPERM';
  }
}

/** Kill `pid` and every descendant, children first. Safe to call on a dead pid. */
export async function killTree(pid: number): Promise<void> {
  if (IS_WIN) {
    await execP('taskkill', ['/PID', String(pid), '/T', '/F']).catch(() => {});
    return;
  }
  // Collect descendants BEFORE signalling: once the parent dies they are re-parented
  // to init and can no longer be found by walking from `pid`.
  let kids: ProcInfo[] = [];
  try {
    kids = descendantsOf(pid, await listProcesses());
  } catch {
    // fall through: the process-group kill below still gets the common case
  }
  for (const sig of ['SIGTERM', 'SIGKILL'] as const) {
    for (const p of [...kids.reverse(), { pid, ppid: 0, cmd: '' }]) {
      try {
        process.kill(p.pid, sig);
      } catch {
        /* already gone */
      }
    }
    try {
      process.kill(-pid, sig); // the whole process group (we spawn detached)
    } catch {
      /* no such group */
    }
    const deadline = Date.now() + (sig === 'SIGTERM' ? 2_000 : 3_000);
    while (Date.now() < deadline && [pid, ...kids.map((k) => k.pid)].some(isAlive)) await sleep(50);
    if (![pid, ...kids.map((k) => k.pid)].some(isAlive)) return;
  }
}

/** Human-readable process tree rooted at `roots` (or every process when empty). */
export async function renderProcessTree(roots: number[]): Promise<string> {
  const procs = await listProcesses();
  const byPid = new Map(procs.map((p) => [p.pid, p]));
  const lines: string[] = [];
  const walk = (p: ProcInfo, depth: number) => {
    lines.push(`${'  '.repeat(depth)}${p.pid} ${p.cmd.slice(0, 300)}`);
    for (const k of procs.filter((c) => c.ppid === p.pid)) walk(k, depth + 1);
  };
  for (const r of roots) {
    const p = byPid.get(r);
    if (p) walk(p, 0);
    else lines.push(`${r} (exited)`);
  }
  return lines.join('\n') + '\n';
}

/** Processes anywhere on the machine whose command line mentions one of `needles`. */
export async function findProcessesMentioning(needles: string[]): Promise<ProcInfo[]> {
  const self = process.pid;
  return (await listProcesses()).filter(
    (p) => p.pid !== self && needles.some((n) => n && p.cmd.includes(n)) && !/\b(ps|powershell|Get-CimInstance)\b.*-axo|Win32_Process/.test(p.cmd),
  );
}

/** Parse `epmd -names` output into registered node names. */
export function parseEpmdNames(out: string): string[] {
  return [...out.matchAll(/^name\s+(\S+)\s+at port\s+\d+/gm)].map((m) => m[1]!);
}

/** Registered epmd node names right now ([] when epmd isn't running or isn't installed). */
export async function epmdNames(env: NodeJS.ProcessEnv): Promise<{ names: string[]; raw: string }> {
  try {
    const { stdout } = await execP('epmd', ['-names'], { env, timeout: 10_000 });
    return { names: parseEpmdNames(stdout), raw: stdout };
  } catch (e) {
    const err = e as { stdout?: string; stderr?: string; message: string };
    return { names: [], raw: `${err.stdout ?? ''}${err.stderr ?? ''}${err.message}` };
  }
}

/** Deregister a leaked node name so one leak doesn't cascade into later tests. */
export async function epmdStop(name: string, env: NodeJS.ProcessEnv): Promise<void> {
  await execP('epmd', ['-stop', name], { env, timeout: 10_000 }).catch(() => {});
}

/** Reject with "step <label> exceeded <s>s" if `fn` doesn't settle in time. */
export async function withHangGuard<T>(label: string, ms: number, fn: () => Promise<T>): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  const guard = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`step \`${label}\` exceeded ${Math.round(ms / 1000)}s (hang guard)`)), ms);
  });
  try {
    return await Promise.race([fn(), guard]);
  } finally {
    clearTimeout(timer);
  }
}
