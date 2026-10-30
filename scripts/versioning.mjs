import semver from 'semver';

const number = '(?:0|[1-9][0-9]*)';
const core = `${number}\\.${number}\\.${number}`;
const semantic = new RegExp(`^v?${core}(?:-[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?$`);
const historical = new RegExp(`^v?(${core})([bf])(?:-([0-9A-Za-z-]+))?$`);

export function parseVersion(value, legacy = true) {
  if (typeof value !== 'string') throw new Error('Version must be a string.');
  value = value.trim();
  if (!semantic.test(value)) {
    const match = legacy && historical.exec(value);
    if (!match) throw new Error(`Invalid semantic version: ${value}`);
    value = `${match[1]}-legacy.${match[2]}.${match[3] || '0'}`;
  }
  return new semver.SemVer(value.startsWith('v') ? value.slice(1) : value);
}

export function coreVersion(value) {
  const parsed = parseVersion(value);
  return `${parsed.major}.${parsed.minor}.${parsed.patch}`;
}

export function normalizedVersion(value) {
  const parsed = parseVersion(value);
  return parsed.version + (parsed.build.length ? `+${parsed.build.join('.')}` : '');
}

export function numericVersion(value) {
  const parsed = parseVersion(value);
  const values = [parsed.major, parsed.minor, parsed.patch, 0];
  if (values.some(value => value > 65535)) throw new Error('Windows version components must be at most 65535.');
  return values.join('.');
}

export function validateReleaseTag(tag) {
  if (typeof tag !== 'string' || !new RegExp(`^v${core}(?:-beta\\.[1-9][0-9]*)?$`).test(tag)) {
    throw new Error('Use vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-beta.N, with N at least 1.');
  }
  const version = parseVersion(tag, false);
  if (semver.lte(coreVersion(tag), '1.2.9')) throw new Error('Future release cores must exceed 1.2.9.');
  return version;
}

export function calculateAlpha(aimed, nearest = null, distance = 0, latestStable = null, runNumber = 1) {
  if (!new RegExp(`^${core}$`).test(aimed)) throw new Error('version.txt must contain a three-number core.');
  if (!Number.isSafeInteger(distance) || distance < 0 || !Number.isSafeInteger(runNumber) || runNumber < 1) {
    throw new Error('Distance must be nonnegative and run number positive.');
  }
  if (!nearest) return `${aimed}-alpha.${runNumber}`;
  const base = parseVersion(nearest);
  if (distance === 0) return base.version;
  let target = aimed;
  if (semver.gt(aimed, coreVersion(nearest))) {
    target = aimed;
  } else if (base.prerelease.length) {
    if (latestStable && semver.gte(coreVersion(latestStable), coreVersion(nearest))) {
      const floor = semver.inc(coreVersion(latestStable), 'patch');
      target = semver.gt(aimed, floor) ? aimed : floor;
    } else {
      return `${base.version}.alpha.${distance}`;
    }
  } else {
    target = semver.inc(coreVersion(nearest), 'patch');
  }
  return `${target}-alpha.${distance}`;
}

export function extractChangelog(text, version) {
  const sections = [...text.matchAll(/^## (.+)\r?$/gm)];
  const matches = sections.filter(match => match[1].trim().startsWith(`${version} - `)
    && /^\d{4}-\d{2}-\d{2}$/.test(match[1].trim().slice(version.length + 3)));
  if (matches.length !== 1) throw new Error(`Expected one dated changelog section for ${version}.`);
  const match = matches[0];
  const next = sections[sections.indexOf(match) + 1];
  const body = text.slice(match.index + match[0].length, next?.index).trim();
  if (!/^[-*] \S/m.test(body)) throw new Error(`Changelog section for ${version} needs a release note.`);
  return body;
}
