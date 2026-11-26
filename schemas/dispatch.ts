// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * Dispatch payload contracts (ADR 0154 § 3).
 *
 * `beamtalk-artifact` is the new `repository_dispatch` payload sent by beamtalk's
 * edge.yml / nightly.yml / release.yml (built by scripts/ci/uat-dispatch-payload.sh).
 * `beamtalk-release` is the legacy `{version, tag}` payload, accepted until the
 * release after Phase 0 ships.
 */
import { z } from 'zod';

export const Sha256 = z.string().regex(/^[0-9a-f]{64}$/, 'expected a lowercase 64-hex sha256');

export const Channel = z.enum(['edge', 'nightly', 'release']);
export type Channel = z.infer<typeof Channel>;

/** An asset file name. It becomes a path on disk, so it must be a bare file name. */
export const AssetName = z
  .string()
  .min(1)
  .refine((n) => !/[\\/]/.test(n) && n !== '.' && n !== '..' && !n.includes('\0'), {
    message: 'asset name must be a bare file name (no path separators)',
  });

export const OtpWindow = z
  .object({
    'min-major': z.number().int().positive(),
    'max-major': z.number().int().positive(),
  })
  .refine((w) => w['min-major'] <= w['max-major'], {
    message: 'otp min-major must be <= max-major',
  });
export type OtpWindow = z.infer<typeof OtpWindow>;

export const ArtifactPayload = z.object({
  channel: Channel,
  version: z.string().min(1),
  tag: z.string().min(1),
  sha: z.string().regex(/^[0-9a-f]{7,40}$/, 'expected a git commit sha'),
  otp: OtpWindow,
  assets: z
    .record(AssetName, Sha256)
    .refine((a) => Object.keys(a).length > 0, { message: 'assets must not be empty' }),
  // Not sent yet (ADR § 10); accepted so the schema doesn't break when it is.
  ide_image: z
    .string()
    .regex(/@sha256:[0-9a-f]{64}$/)
    .optional(),
});
export type ArtifactPayload = z.infer<typeof ArtifactPayload>;

/** Legacy `beamtalk-release` payload: `{ "version": "0.4.0", "tag": "v0.4.0" }`. */
export const LegacyReleasePayload = z.object({
  version: z.string().min(1),
  tag: z.string().regex(/^v\d/, 'release tag must look like vX.Y.Z'),
});
export type LegacyReleasePayload = z.infer<typeof LegacyReleasePayload>;
