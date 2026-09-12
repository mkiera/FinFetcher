import re
from pathlib import Path


_NUMBER = r'(?:0|[1-9][0-9]*)'
_CORE = rf'({_NUMBER})\.({_NUMBER})\.({_NUMBER})'
_SEMVER = re.compile(rf'v?{_CORE}(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?')
_LEGACY = re.compile(rf'v?{_CORE}([bf])(?:-([0-9A-Za-z-]+))?')
_RELEASE = re.compile(rf'v{_CORE}(?:-beta\.([1-9][0-9]*))?')
LAST_HISTORICAL_CORE = (1, 2, 9)


def parse_version(value, *, legacy=True):
    if not isinstance(value, str):
        return None
    value = value.strip()
    match = _SEMVER.fullmatch(value)
    if match:
        identifiers = match[4].split('.') if match[4] else []
        if any(i.isdigit() and len(i) > 1 and i.startswith('0') for i in identifiers):
            return None
    elif legacy and (match := _LEGACY.fullmatch(value)):
        identifiers = ['legacy', match[4], match[5] or '0']
    else:
        return None
    core = tuple(int(match[i]) for i in (1, 2, 3))
    tail = tuple((0, int(i)) if i.isdigit() else (1, i) for i in identifiers)
    return (*core, int(not identifiers), tail)


def is_newer(remote, local):
    remote, local = parse_version(remote), parse_version(local)
    return remote is not None and local is not None and remote > local


def release_version(tag):
    match = _RELEASE.fullmatch(tag)
    if not match:
        raise ValueError('Use vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-beta.N, with N >= 1')
    if parse_version(tag)[:3] <= LAST_HISTORICAL_CORE:
        raise ValueError('Future releases must have a core above 1.2.9. Historical versions are immutable.')
    return tag[1:]


def alpha_version(aimed, nearest, distance, latest_stable, run_number):
    target = parse_version(aimed, legacy=False)
    if target is None or not re.fullmatch(_CORE, aimed):
        raise ValueError('version.txt must contain a three-number core')
    if distance < 0 or run_number < 1:
        raise ValueError('Distance must be nonnegative and run number must be positive')
    core = target[:3]
    base = parse_version(nearest) if nearest else None
    if nearest and base is None:
        raise ValueError(f'Invalid nearest tag: {nearest}')
    if base is None:
        return f'{aimed}-alpha.{run_number}'
    base_core = '.'.join(map(str, base[:3]))
    prerelease = '.'.join(str(i[1]) for i in base[4])
    if distance == 0:
        return base_core + (f'-{prerelease}' if prerelease else '')
    stable = parse_version(latest_stable, legacy=False) if latest_stable else None
    if core > base[:3]:
        pass
    elif not base[3] and stable and stable[:3] >= base[:3]:
        core = max(core, (*stable[:2], stable[2] + 1))
    elif not base[3]:
        return f'{base_core}-{prerelease}.alpha.{distance}'
    else:
        core = (*base[:2], base[2] + 1)
    return '.'.join(map(str, core)) + f'-alpha.{distance}'


def read_build_version(root, override=None):
    version = override if override is not None else (Path(root) / 'version.txt').read_text(encoding='utf-8').strip()
    if parse_version(version) is None:
        raise ValueError(f'Invalid build version: {version}')
    return version.removeprefix('v')


def changelog_section(text, version):
    sections = list(re.finditer(r'^## (.+)\r?$', text, re.MULTILINE))
    matches = [(i, m) for i, m in enumerate(sections)
               if re.fullmatch(re.escape(version) + r' - \d{4}-\d{2}-\d{2}', m[1].strip())]
    if len(matches) != 1:
        raise ValueError(f'Expected one dated changelog section for {version}')
    index, match = matches[0]
    end = sections[index + 1].start() if index + 1 < len(sections) else len(text)
    body = text[match.end():end].strip()
    if not re.search(r'^[-*] \S', body, re.MULTILINE):
        raise ValueError(f'Changelog section for {version} needs a release note')
    return body
