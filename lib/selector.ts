// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

/** The version selector: latest | edge | nightly | vX.Y.Z | local:<path> (ADR 0154 § 4). */
export type Selector =
  | { kind: 'latest' }
  | { kind: 'edge'; tag: 'edge' }
  | { kind: 'nightly'; tag: 'nightly' }
  | { kind: 'version'; tag: string }
  | { kind: 'local'; path: string };

const VERSION_RE = /^v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)$/;

export function parseSelector(raw: string): Selector {
  const s = raw.trim();
  if (s === '' || s === 'latest') return { kind: 'latest' };
  if (s === 'edge') return { kind: 'edge', tag: 'edge' };
  if (s === 'nightly') return { kind: 'nightly', tag: 'nightly' };
  if (s.startsWith('local:')) {
    const path = s.slice('local:'.length);
    if (!path) throw new Error('selector `local:` needs a path to a beamtalk binary');
    return { kind: 'local', path };
  }
  const m = VERSION_RE.exec(s);
  if (m) return { kind: 'version', tag: `v${m[1]}` };
  throw new Error(
    `invalid version selector ${JSON.stringify(raw)}: expected latest|edge|nightly|vX.Y.Z|local:<path>`,
  );
}

export function describeSelector(s: Selector): string {
  switch (s.kind) {
    case 'latest':
    case 'edge':
    case 'nightly':
      return s.kind;
    case 'version':
      return s.tag;
    case 'local':
      return `local:${s.path}`;
  }
}
