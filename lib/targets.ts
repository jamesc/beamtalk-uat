// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/** Loader for the owner-gated targets.toml. */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parse } from 'smol-toml';
import { z } from 'zod';
import { Channel, TargetName } from '../schemas/index.ts';

const TargetsFile = z.object({
  targets: z.partialRecord(
    TargetName,
    z.object({ runner: z.string().min(1), required: z.array(Channel) }),
  ),
  leg: z.array(
    z.object({
      target: TargetName,
      otp: z.enum(['min', 'max']),
      channels: z.array(Channel).min(1),
      grep: z.string().optional(),
      shards: z.number().int().min(1).default(1),
    }),
  ),
});
export type TargetsFile = z.infer<typeof TargetsFile>;

export const TARGETS_TOML = fileURLToPath(new URL('../targets.toml', import.meta.url));

export function parseTargets(text: string): TargetsFile {
  return TargetsFile.parse(parse(text));
}

export function loadTargets(path = TARGETS_TOML): TargetsFile {
  return parseTargets(readFileSync(path, 'utf8'));
}

/** The target the current machine is. */
export function hostTarget(): TargetName {
  const key = `${process.platform}-${process.arch}`;
  switch (key) {
    case 'linux-x64':
      return 'linux-x86_64';
    case 'darwin-arm64':
      return 'macos-arm64';
    case 'darwin-x64':
      return 'macos-x86_64';
    case 'win32-x64':
      return 'windows-x86_64';
    default:
      throw new Error(`unsupported platform for the Beamtalk bundle: ${key}`);
  }
}

export function archiveExt(target: TargetName): 'tar.gz' | 'zip' {
  return target === 'windows-x86_64' ? 'zip' : 'tar.gz';
}
