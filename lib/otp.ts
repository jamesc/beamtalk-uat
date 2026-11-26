// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * Erlang/OTP provisioning (ADR 0154 § 1, § 9).
 *
 * Two ways OTP gets onto a runner:
 *  - every test except J1: `erlef/setup-beam` installs one OTP per major (gate.yml
 *    keys its tool cache by target + major); the OTP install itself is not under test.
 *  - J1: performs the *documented* install for the target (`documentedInstall`),
 *    because that install is part of what a user has.
 */
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import type { TargetName } from '../schemas/index.ts';

const exec = promisify(execFile);

/** The OTP major of the `erl` on PATH (or of `env.PATH`), e.g. 28. */
export async function detectOtpMajor(env: NodeJS.ProcessEnv = process.env): Promise<number> {
  const { stdout } = await exec(
    'erl',
    ['-noshell', '-eval', 'io:format("~s",[erlang:system_info(otp_release)]),halt().'],
    { env, timeout: 60_000 },
  );
  const major = Number.parseInt(stdout.trim(), 10);
  if (!Number.isInteger(major)) throw new Error(`could not parse OTP release from ${JSON.stringify(stdout)}`);
  return major;
}

/** Fail fast if the runner's OTP is not the major the matrix leg asked for. */
export function assertOtpMajor(actual: number, expected: number | undefined): void {
  if (expected !== undefined && actual !== expected) {
    throw new Error(`OTP major mismatch: leg expects ${expected}, erl on PATH is ${actual}`);
  }
}

export interface InstallStep {
  cmd: string;
  args: string[];
  /** Human-readable description for the transcript. */
  note: string;
}

/**
 * The documented OTP install for a target, as data (J1 runs it; harness tests
 * pin it). Versions are an OTP major; the helper resolves the newest patch.
 */
export function documentedInstall(target: TargetName, major: number): InstallStep[] {
  switch (target) {
    case 'macos-arm64':
    case 'macos-x86_64':
      // brew installs the newest erlang; versioned formulae exist as erlang@<major>.
      return [{ cmd: 'brew', args: ['install', `erlang@${major}`], note: `brew install erlang@${major}` }];
    case 'windows-x86_64':
      return [
        {
          cmd: 'powershell',
          args: [
            '-NoProfile',
            '-Command',
            `winget install --id Erlang.ErlangOTP --silent --accept-package-agreements --accept-source-agreements --version ${major}`,
          ],
          note: `erlang.org installer for OTP ${major} (via winget)`,
        },
      ];
    case 'linux-x86_64':
      // `apt install erlang` is older than the supported window on ubuntu-24.04
      // (ADR 0154 § 1), so J1 uses the asdf method and files the apt docs Bug.
      return [
        { cmd: 'asdf', args: ['plugin', 'add', 'erlang'], note: 'asdf plugin add erlang' },
        { cmd: 'asdf', args: ['install', 'erlang', `latest:${major}`], note: `asdf install erlang latest:${major}` },
      ];
  }
}
