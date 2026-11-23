#!/usr/bin/env node
// Generate the site's release-notes page from the repository CHANGELOG.md.
//
// release-please owns CHANGELOG.md, so the website should not keep a second,
// hand-edited copy. This script runs before `dev`, `build`, and `check` (via
// npm's pre* hooks) and writes the page Starlight serves at /release-notes/.

import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const website = resolve(here, '..');
const repo = resolve(website, '..');

const changelog = await readFile(resolve(repo, 'CHANGELOG.md'), 'utf8');

const body = changelog
  .replace(/^# Changelog\s*\n/, '')
  .replace(/^\s+/, '')
  .trimEnd();

const page = `---
title: Release notes
description: What changed in each abmx release.
---

Released binaries and their notes are also published on
[GitHub Releases](https://github.com/kennethlieyanto/abmx/releases).

${body}
`;

const out = resolve(website, 'src/content/docs/release-notes.md');
await mkdir(dirname(out), { recursive: true });
await writeFile(out, page, 'utf8');

console.log(`Release notes written to ${out}`);
