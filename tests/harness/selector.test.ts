// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
import { expect, test } from '@playwright/test';
import { parseSelector } from '../../lib/selector.ts';

test('selector forms', () => {
  expect(parseSelector('')).toEqual({ kind: 'latest' });
  expect(parseSelector('latest')).toEqual({ kind: 'latest' });
  expect(parseSelector('edge')).toEqual({ kind: 'edge', tag: 'edge' });
  expect(parseSelector('nightly')).toEqual({ kind: 'nightly', tag: 'nightly' });
  expect(parseSelector('v0.4.0')).toEqual({ kind: 'version', tag: 'v0.4.0' });
  expect(parseSelector('0.4.0')).toEqual({ kind: 'version', tag: 'v0.4.0' });
  expect(parseSelector('local:/opt/bt/bin/beamtalk')).toEqual({ kind: 'local', path: '/opt/bt/bin/beamtalk' });
  expect(parseSelector('local:C:\\bt\\beamtalk.exe')).toEqual({ kind: 'local', path: 'C:\\bt\\beamtalk.exe' });
});

test('invalid selectors are rejected', () => {
  for (const bad of ['stable', 'v1', 'local:', '1.2', '../x']) expect(() => parseSelector(bad), bad).toThrow();
});
