import argparse
import os
import subprocess
from pathlib import Path

from versioning import alpha_version, changelog_section, parse_version, release_version


def git(*args):
    return subprocess.check_output(('git', *args), text=True).strip()


def calculate_alpha(aimed, run_number):
    tags = [tag for tag in git('tag', '--list', 'v[0-9]*').splitlines() if parse_version(tag)]
    stable_tags = [tag for tag in tags if parse_version(tag, legacy=False)
                   and parse_version(tag)[3] and '+' not in tag]
    latest = max(stable_tags, key=parse_version, default=None)
    nearest, distance = None, 0
    if tags:
        try:
            describe = git('describe', '--tags', '--long', *[arg for tag in tags for arg in ('--match', tag)])
            nearest, distance, _sha = describe.rsplit('-', 2)
            distance = int(distance)
        except subprocess.CalledProcessError:
            pass
    return alpha_version(aimed, nearest, distance, latest, run_number)


def validate_placement(tag):
    version = release_version(tag)
    commit = git('rev-parse', f'{tag}^{{commit}}')
    branch = 'beta' if '-' in version else 'main'
    if commit != git('rev-parse', f'origin/{branch}'):
        raise ValueError(f'{tag} must point to the current remote {branch} head')
    if branch == 'main':
        parents = git('rev-list', '--parents', '-n', '1', commit).split()[1:]
        if len(parents) != 2 or parents[1] != git('rev-parse', 'origin/beta'):
            raise ValueError('Stable tags must point to a release merge from beta')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('kind', choices=('release', 'alpha'))
    parser.add_argument('--tag')
    parser.add_argument('--run-number', type=int, default=1)
    parser.add_argument('--validate-placement', action='store_true')
    args = parser.parse_args()
    if args.kind == 'release':
        version = release_version(args.tag or '')
        if args.validate_placement:
            validate_placement(args.tag)
        notes = changelog_section(Path('CHANGELOG.md').read_text(encoding='utf-8'), version)
        Path('build').mkdir(exist_ok=True)
        Path('build/release-notes.md').write_text(
            notes + '\n\n<!-- app-notes-end -->\n\n'
            'Run FinFetcher-Setup.exe to install or update.\n'
            'FinFetcher-Legacy.exe supports updates from older portable copies.\n', encoding='utf-8')
    else:
        aimed = Path('version.txt').read_text(encoding='utf-8').strip()
        version = calculate_alpha(aimed, args.run_number)
    destination = Path('build/version')
    destination.mkdir(parents=True, exist_ok=True)
    (destination / 'version.txt').write_text(version, encoding='utf-8')
    outputs = {'VERSION': version, 'VERNUM': '.'.join(map(str, (*parse_version(version)[:3], 0))),
               'IS_BETA': str(not bool(parse_version(version)[3])).lower()}
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
            for key, value in outputs.items():
                output.write(f'{key}={value}\n')
    print(version)


if __name__ == '__main__':
    main()
