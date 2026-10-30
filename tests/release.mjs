import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
import { calculateAlpha, extractChangelog, normalizedVersion, numericVersion, parseVersion, validateReleaseTag } from '../scripts/versioning.mjs';
import { calculateRepositoryAlpha, stampIdentity, validateTagPlacement } from '../scripts/build-identity.mjs';
import { buildFrontend, frontendFiles } from '../scripts/build-frontend.mjs';

test('historical versions and future release rules retain their exact meaning', () => {
  for (const version of ['v1.0.1f-streaming', 'v1.2.3b-bundle-certifi', 'v1.2.4b', 'v1.2.9f-flipperclipper']) {
    assert.ok(parseVersion(version).compare(parseVersion('1.2.10-beta.1')) < 0);
  }
  for (const invalid of ['1', '1.2', '01.2.3', '1.2.3-beta.01', 'vv1.2.3', '1.2.3-', '1.2.3+bad_tail']) {
    assert.throws(() => parseVersion(invalid));
  }
  for (const tag of ['v1.2.9', 'v1.2.9-beta.1', '1.2.10', 'v1.2.10b', 'v1.2.10-beta.0', 'v1.2.10-beta.01', 'v1.2.10-alpha.1', 'v1.2.10+build']) {
    assert.throws(() => validateReleaseTag(tag));
  }
  assert.equal(validateReleaseTag('v1.2.10-beta.11').version, '1.2.10-beta.11');
  assert.equal(normalizedVersion('v1.2.10+abc.01'), '1.2.10+abc.01');
  assert.equal(numericVersion('1.2.10-beta.11'), '1.2.10.0');
});

test('alpha versions match previous release rules including legacy tags and stable merges', () => {
  const cases = [
    [['0.1.0', null, 0, null, 17], '0.1.0-alpha.17'],
    [['1.4.0', 'v1.4.0-beta.1', 0, 'v1.3.2', 1], '1.4.0-beta.1'],
    [['1.4.0', 'v1.4.0-beta.1', 3, 'v1.3.2', 1], '1.4.0-beta.1.alpha.3'],
    [['1.4.0', 'v1.4.0', 2, 'v1.4.0', 1], '1.4.1-alpha.2'],
    [['1.5.0', 'v1.4.0', 2, 'v1.4.0', 1], '1.5.0-alpha.2'],
    [['1.4.0', 'v1.4.0-beta.1', 2, 'v1.4.0', 1], '1.4.1-alpha.2'],
    [['1.2.10', 'v1.2.9f-flipperclipper', 3, 'v1.2.9', 1], '1.2.10-alpha.3'],
    [['1.2.9', 'v1.2.9f-flipperclipper', 0, 'v1.2.9', 1], '1.2.9-legacy.f.flipperclipper'],
  ];
  for (const [args, expected] of cases) assert.equal(calculateAlpha(...args), expected);
});

test('changelog extraction requires one exact dated section containing a note', () => {
  const text = '# Changelog\n\n## Unreleased\n\n## 1.2.10-beta.1 - 2026-09-25\n\n- Fixed downloads.\n\n## 1.2.9 - 2026-08-01\n\n- Old.\n';
  assert.equal(extractChangelog(text, '1.2.10-beta.1'), '- Fixed downloads.');
  assert.throws(() => extractChangelog(text, '1.2.10'));
  assert.throws(() => extractChangelog(text + text, '1.2.10-beta.1'));
  assert.throws(() => extractChangelog('## 1.2.10 - 2026-09-25\n', '1.2.10'));
});

test('stable release tags must target the remote main merge containing remote beta', () => {
  const values = { 'v1.2.10^{commit}': 'merge', 'origin/main': 'merge', 'origin/beta': 'beta' };
  const readGit = args => args[0] === 'rev-parse' ? values[args[1]] : 'merge old-main beta';
  validateTagPlacement('v1.2.10', readGit);
  values['origin/beta'] = 'unreleased';
  assert.throws(() => validateTagPlacement('v1.2.10', readGit));
});

test('alpha calculation uses stable tags on another branch without moving any tags', () => {
  const root = mkdtempSync(join(tmpdir(), 'FinFetcher-version-test-'));
  const git = args => execFileSync('git', args, { cwd: root, encoding: 'utf8', windowsHide: true }).trim();
  git(['init', '-b', 'beta']);
  git(['config', 'user.name', 'Test']);
  git(['config', 'user.email', 'test@example.test']);
  git(['commit', '--allow-empty', '-m', 'Initial']);
  git(['tag', 'v1.4.0-beta.1']);
  git(['branch', 'main']);
  git(['commit', '--allow-empty', '-m', 'Fix']);
  git(['checkout', 'main']);
  git(['merge', '--no-ff', 'beta', '-m', 'Release']);
  git(['tag', 'v1.4.0']);
  git(['checkout', 'beta']);
  git(['commit', '--allow-empty', '-m', 'Next fix']);
  assert.equal(calculateRepositoryAlpha('1.4.0', 9, git), '1.4.1-alpha.2');
});

test('version stamping leaves source version untouched and aligns binary, installer, and identity', () => {
  const root = mkdtempSync(join(tmpdir(), 'FinFetcher-stamp-test-'));
  writeFileSync(join(root, 'version.txt'), '1.2.10\n');
  const identity = stampIdentity({ version: '1.2.10-beta.11' }, root, { GITHUB_SHA: 'workflow-commit' }, args => args[0] === 'rev-parse' ? 'abc123' : 'rewrite');
  assert.equal(readFileSync(join(root, 'version.txt'), 'utf8'), '1.2.10\n');
  assert.equal(readFileSync(join(root, 'build/version/version.txt'), 'utf8').trim(), identity.version);
  assert.equal(JSON.parse(readFileSync(join(root, 'src-tauri/build-config.json'), 'utf8')).version, identity.version);
  assert.equal(JSON.parse(readFileSync(join(root, 'build_info.json'), 'utf8')).sha, 'abc123');
});

test('frontend staging contains only application assets and replaces stale build output', () => {
  const root = mkdtempSync(join(tmpdir(), 'FinFetcher-frontend-test-'));
  mkdirSync(join(root, 'fonts'));
  for (const file of [...frontendFiles, 'fonts/outfit.woff2']) writeFileSync(join(root, file), 'asset');
  writeFileSync(join(root, 'private-notes.md'), 'private');
  buildFrontend(root);
  writeFileSync(join(root, 'dist-frontend/stale.js'), 'old');
  const included = buildFrontend(root);
  assert.ok(included.includes('desktop.js'));
  assert.ok(included.includes('fonts/outfit.woff2'));
  assert.ok(!existsSync(join(root, 'dist-frontend/private-notes.md')));
  assert.ok(!existsSync(join(root, 'dist-frontend/stale.js')));
});
