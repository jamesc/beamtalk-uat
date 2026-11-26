// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
//
// Archive extraction on the OS it runs on. The Windows bundle is a .zip and the
// `tar` on a Windows runner's PATH can be Git's GNU tar (cannot read zip), so this
// runs in ci.yml on windows-2022 as well as Linux.

import { expect, test } from '@playwright/test';
import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
// (.tar.gz extraction is covered on POSIX by gate-e2e's install-from-artifact test.)
import { extractArchive } from '../../lib/toolchain.ts';

const exec = promisify(execFile);

async function sample(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), 'btuat-ex-'));
  await mkdir(join(dir, 'src', 'beamtalk-1.0.0', 'bin'), { recursive: true });
  await writeFile(join(dir, 'src', 'beamtalk-1.0.0', 'bin', 'beamtalk'), 'hello bundle');
  return dir;
}

test('a .zip bundle extracts on this OS', async () => {
  const dir = await sample();
  const zip = join(dir, 'bundle.zip');
  if (process.platform === 'win32') {
    await exec('powershell', ['-NoProfile', '-Command', `Compress-Archive -Path '${join(dir, 'src', 'beamtalk-1.0.0')}' -DestinationPath '${zip}'`]);
  } else {
    await exec('zip', ['-qr', zip, 'beamtalk-1.0.0'], { cwd: join(dir, 'src') });
  }
  const out = join(dir, 'out');
  await extractArchive(zip, out);
  expect(await readFile(join(out, 'beamtalk-1.0.0', 'bin', 'beamtalk'), 'utf8')).toBe('hello bundle');
});
