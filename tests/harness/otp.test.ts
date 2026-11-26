// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
import { expect, test } from '@playwright/test';
import { assertOtpMajor, documentedInstall } from '../../lib/otp.ts';

test('a leg whose erl is the wrong OTP major fails fast', () => {
  expect(() => assertOtpMajor(27, 28)).toThrow(/expects 28, erl on PATH is 27/);
  expect(() => assertOtpMajor(28, 28)).not.toThrow();
  expect(() => assertOtpMajor(28, undefined)).not.toThrow();
});

test('documented OTP install per target (J1 helper; ADR 0154 § 1)', () => {
  const line = (t: Parameters<typeof documentedInstall>[0]) => documentedInstall(t, 28).map((s) => s.note).join(' && ');
  expect(line('macos-arm64')).toBe('brew install erlang@28');
  expect(line('macos-x86_64')).toBe('brew install erlang@28');
  expect(line('linux-x86_64')).toContain('asdf install erlang latest:28');
  expect(line('linux-x86_64')).not.toContain('apt'); // apt gives an OTP older than the window
  expect(line('windows-x86_64')).toContain('erlang.org installer');
});
