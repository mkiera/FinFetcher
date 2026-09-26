import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';
import { parseArgs } from 'node:util';
import semver from 'semver';
import { calculateAlpha, extractChangelog, normalizedVersion, numericVersion, parseVersion, validateReleaseTag } from './versioning.mjs';

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

export function git(args, required = true, root = projectRoot) {
  const result = spawnSync('git', args, { cwd: root, encoding: 'utf8', windowsHide: true });
  if (result.status !== 0 && required) throw new Error(result.stderr?.trim() || 'Git command failed.');
  return result.status === 0 ? result.stdout.trim() : '';
}

export function validateTagPlacement(tag, readGit = args => git(args)) {
  const version = validateReleaseTag(tag);
  const target = readGit(['rev-parse', `${tag}^{commit}`]);
  const branch = version.prerelease.length ? 'beta' : 'main';
  if (target !== readGit(['rev-parse', `origin/${branch}`])) throw new Error(`Release tag must point at the remote ${branch} head.`);
  if (branch === 'main') {
    const parents = readGit(['rev-list', '--parents', '-n', '1', target]).split(/\s+/).slice(1);
    if (parents.length !== 2 || parents[1] !== readGit(['rev-parse', 'origin/beta'])) {
      throw new Error('Stable tags must point to the release merge from beta.');
    }
  }
}

export function calculateRepositoryAlpha(aimed, runNumber, readGit = (args, required = true) => git(args, required)) {
  const tags = readGit(['tag', '--list', 'v[0-9]*']).split(/\r?\n/).filter(tag => {
    try { parseVersion(tag); return true; } catch { return false; }
  });
  const stable = tags.filter(tag => {
    try { return parseVersion(tag, false).prerelease.length === 0 && !tag.includes('+'); } catch { return false; }
  }).sort(semver.rcompare)[0];
  const describe = tags.length ? readGit(['describe', '--tags', '--long', ...tags.flatMap(tag => ['--match', tag])], false) : '';
  const nearest = /^(.+)-(\d+)-g[0-9a-f]+$/.exec(describe);
  return calculateAlpha(aimed, nearest?.[1], nearest ? Number(nearest[2]) : 0, stable, runNumber);
}

export function stampIdentity(args = {}, root = projectRoot, environment = process.env, readGit = (values, required = true) => git(values, required, root)) {
  const aimed = readFileSync(resolve(root, 'version.txt'), 'utf8').trim();
  let version = args.version || aimed;
  if (args.tag) {
    version = validateReleaseTag(args.tag).version;
    if (version.split('-')[0] !== aimed) throw new Error('Release version core must match version.txt.');
    if (args['validate-placement']) validateTagPlacement(args.tag, readGit);
    const notes = extractChangelog(readFileSync(resolve(root, 'CHANGELOG.md'), 'utf8'), version);
    mkdirSync(resolve(root, 'build'), { recursive: true });
    writeFileSync(resolve(root, 'build/release-notes.md'), `${notes}\n\n<!-- app-notes-end -->\n\nRun FinFetcher-Setup.exe to install or update.\n`);
  } else if (args.alpha) {
    version = calculateRepositoryAlpha(aimed, Number(args['run-number'] || environment.GITHUB_RUN_NUMBER || 1), readGit);
  }
  version = normalizedVersion(version);
  const identity = {
    version,
    sha: readGit(['rev-parse', 'HEAD'], false) || environment.GITHUB_SHA || '',
    branch: args.branch || environment.GITHUB_HEAD_REF || environment.GITHUB_REF_NAME || readGit(['branch', '--show-current'], false),
    run_id: String(args['run-id'] || environment.GITHUB_RUN_ID || ''),
    built_at: new Date().toISOString(),
    ytdlp: null,
  };
  const windowsVersion = numericVersion(version);
  mkdirSync(resolve(root, 'build/version'), { recursive: true });
  mkdirSync(resolve(root, 'src-tauri'), { recursive: true });
  writeFileSync(resolve(root, 'build/version/version.txt'), `${version}\n`);
  writeFileSync(resolve(root, 'build_info.json'), `${JSON.stringify(identity, null, 2)}\n`);
  writeFileSync(resolve(root, 'src-tauri/build-config.json'), `${JSON.stringify({ productName: 'FinFetcher', version }, null, 2)}\n`);
  if (environment.GITHUB_OUTPUT) appendFileSync(environment.GITHUB_OUTPUT, `VERSION=${version}\nVERNUM=${windowsVersion}\nIS_BETA=${parseVersion(version).prerelease.length > 0}\n`);
  return identity;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const { values } = parseArgs({ options: {
      version: { type: 'string' }, tag: { type: 'string' }, alpha: { type: 'boolean' },
      'validate-placement': { type: 'boolean' }, branch: { type: 'string' },
      'run-id': { type: 'string' }, 'run-number': { type: 'string' },
    } });
    if ([values.version, values.tag, values.alpha].filter(Boolean).length > 1) throw new Error('Choose one version source.');
    console.log(JSON.stringify(stampIdentity(values)));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
