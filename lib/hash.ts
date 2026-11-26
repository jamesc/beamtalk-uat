// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';

export async function sha256File(path: string): Promise<{ sha256: string; size: number }> {
  const h = createHash('sha256');
  await new Promise<void>((resolve, reject) => {
    createReadStream(path)
      .on('data', (c) => h.update(c))
      .on('error', reject)
      .on('end', () => resolve());
  });
  return { sha256: h.digest('hex'), size: (await stat(path)).size };
}
