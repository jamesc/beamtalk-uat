// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * Install the toolchain under test and write .uat/toolchain.json for the Playwright
 * workers. Runs once per job, BEFORE `playwright test`, so every shard sees the same
 * install (a Playwright setup project would run per shard).
 *
 *   BEAMTALK_UAT_ARTIFACT_DIR  resolve artifact dir (CI shards) -- wins over the selector
 *   BEAMTALK_UAT_VERSION       latest|edge|nightly|vX.Y.Z|local:<path>   (default: latest)
 *   BEAMTALK_UAT_OTP           OTP major the leg expects; erl on PATH must match
 */
import { appendFileSync } from 'node:fs';
import { NotPublishedError } from '../lib/resolve.ts';
import { assertOtpMajor, detectOtpMajor } from '../lib/otp.ts';
import { prepareToolchain } from '../lib/toolchain.ts';

let info;
try {
  info = await prepareToolchain();
} catch (e) {
  if (e instanceof NotPublishedError) {
    // A rolling release that is not published yet is a skip, not a failure (BT-2497).
    console.log(`::notice title=skipped::${e.message}`);
    if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, 'skipped=true\n');
    process.exit(0);
  }
  throw e;
}
const major = await detectOtpMajor();
assertOtpMajor(major, info.otpMajor);
console.log(`toolchain: ${info.version}  (${info.selector})`);
console.log(`bin:       ${info.bin}`);
console.log(`otp:       ${major}`);
if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, 'skipped=false\n');
