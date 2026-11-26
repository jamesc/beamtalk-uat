// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * The manifest written by the `resolve` job next to the downloaded assets. Every
 * shard installs from this (and re-checks the hashes), never from the network.
 */
import { z } from 'zod';
import { AssetName, Channel, OtpWindow, Sha256 } from './dispatch.ts';

export const TargetName = z.enum([
  'linux-x86_64',
  'macos-arm64',
  'macos-x86_64',
  'windows-x86_64',
]);
export type TargetName = z.infer<typeof TargetName>;

export const Manifest = z.object({
  schemaVersion: z.literal(1),
  source: z.enum(['payload', 'legacy', 'selector']),
  channel: Channel,
  version: z.string().min(1),
  tag: z.string().min(1),
  sha: z.string().optional(),
  otp: OtpWindow,
  assets: z.record(AssetName, z.object({ sha256: Sha256, size: z.number().int().nonnegative() })),
  /** target -> toolchain archive name (a key of `assets`) */
  toolchains: z.partialRecord(TargetName, AssetName),
  /** Assets with no independent hash to verify against (selector path, no digest). */
  unverified: z.array(AssetName),
});
export type Manifest = z.infer<typeof Manifest>;

export const ToolchainInfo = z.object({
  bin: z.string().min(1),
  version: z.string().min(1),
  prefix: z.string().optional(),
  selector: z.string(),
  target: z.string().optional(),
  otpMajor: z.number().int().optional(),
});
export type ToolchainInfo = z.infer<typeof ToolchainInfo>;
