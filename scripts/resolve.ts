// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/**
 * The `resolve` job's entry point (gate.yml). Reads the trigger from the
 * environment, downloads every asset once, verifies the sha256s and writes the
 * workflow artifact directory plus the GITHUB_OUTPUT values the fan-out needs.
 *
 *   EVENT_NAME      github.event_name
 *   EVENT_ACTION    github.event.action          (repository_dispatch type)
 *   PAYLOAD_JSON    toJSON(github.event.client_payload)
 *   SELECTOR        workflow_dispatch version input (default: latest)
 *   OUT_DIR         where to write assets/ + manifest.json (default: resolved)
 */
import { appendFileSync } from 'node:fs';
import { githubSource } from '../lib/github.ts';
import { SupersededError, buildMatrix, inputFromEvent, resolve } from '../lib/resolve.ts';

function output(k: string, v: string): void {
  console.log(`${k}=${v}`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${k}=${v}\n`);
}

async function main(): Promise<void> {
  const outDir = process.env.OUT_DIR || 'resolved';
  const input = inputFromEvent({
    eventName: process.env.EVENT_NAME || 'workflow_dispatch',
    eventAction: process.env.EVENT_ACTION,
    payload: process.env.PAYLOAD_JSON ? JSON.parse(process.env.PAYLOAD_JSON) : undefined,
    selector: process.env.SELECTOR,
  });
  try {
    const manifest = await resolve(input, githubSource, outDir);
    const matrix = buildMatrix(manifest);
    output('superseded', 'false');
    output('channel', manifest.channel);
    output('version', manifest.version);
    output('tag', manifest.tag);
    output('matrix', JSON.stringify({ include: matrix }));
    if (manifest.unverified.length) {
      console.log(`::warning::no independent sha256 for: ${manifest.unverified.join(', ')}`);
    }
  } catch (e) {
    if (e instanceof SupersededError) {
      console.log(`::notice title=superseded::${e.message}`);
      output('superseded', 'true');
      output('matrix', JSON.stringify({ include: [] }));
      return; // exit 0: the workflow cancels the run; nothing is filed
    }
    throw e;
  }
}

main().catch((e) => {
  console.error(`::error::resolve failed: ${(e as Error).message}`);
  process.exit(1);
});
