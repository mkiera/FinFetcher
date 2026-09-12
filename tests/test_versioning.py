import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from prepare_version import calculate_alpha, validate_placement
from versioning import alpha_version, changelog_section, is_newer, parse_version, read_build_version, release_version


class VersionTests(unittest.TestCase):
    def test_semver_precedence(self):
        versions = ['1.2.4', '1.2.5-alpha.1', '1.2.5-beta.1',
                    '1.2.5-beta.1.alpha.1', '1.2.5-beta.2', '1.2.5-beta.11', '1.2.5']
        for older, newer in zip(versions, versions[1:]):
            with self.subTest(newer=newer):
                self.assertTrue(is_newer(newer, older))
                self.assertFalse(is_newer(older, newer))
        self.assertEqual(parse_version('v1.2.5+abc.01'), parse_version('1.2.5+def'))
        self.assertTrue(is_newer('1.0.0-alpha.a', '1.0.0-alpha.999'))

    def test_invalid_versions_are_rejected(self):
        for version in ('1', '1.2', '1.2.3.4', '-1.2.3', '01.2.3', '1.02.3',
                        '1.2.3-beta.01', '1.2.3-', '1.2.3-beta..1', '1.2.3+',
                        'vv1.2.3', '1.2.3-beta_1', '1.2.3+bad_tail', None):
            with self.subTest(version=version):
                self.assertIsNone(parse_version(version))
        self.assertFalse(is_newer('invalid', '1.2.9'))

    def test_historical_versions_remain_readable(self):
        tags = ('v1.0.0', 'v1.0.1f-streaming', 'v1.1.0', 'v1.2.0', 'v1.2.1',
                'v1.2.2', 'v1.2.3', 'v1.2.3b-bundle-certifi', 'v1.2.4', 'v1.2.4b',
                'v1.2.4f-installer', 'v1.2.5', 'v1.2.6', 'v1.2.7', 'v1.2.8',
                'v1.2.9', 'v1.2.9f-flipperclipper')
        for tag in tags:
            with self.subTest(tag=tag):
                self.assertIsNotNone(parse_version(tag))
                self.assertTrue(is_newer('1.2.10-beta.1', tag))
        self.assertTrue(is_newer('1.2.4', '1.2.4b'))

    def test_future_tags_are_strict_and_above_history(self):
        self.assertEqual(release_version('v1.2.10-beta.1'), '1.2.10-beta.1')
        self.assertEqual(release_version('v2.0.0'), '2.0.0')
        for tag in ('v1.2.9', 'v1.2.9-beta.1', '1.2.10', 'v1.2.10b',
                    'v1.2.10-beta.0', 'v1.2.10-beta.01', 'v1.2.10-alpha.1',
                    'v1.2.10-rc.1', 'v1.2.10+build'):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release_version(tag)

    def test_alpha_examples(self):
        cases = [
            (('0.1.0', None, 0, None, 17), '0.1.0-alpha.17'),
            (('1.4.0', 'v1.4.0-beta.1', 0, 'v1.3.2', 1), '1.4.0-beta.1'),
            (('1.4.0', 'v1.4.0-beta.1', 3, 'v1.3.2', 1), '1.4.0-beta.1.alpha.3'),
            (('1.4.0', 'v1.4.0', 2, 'v1.4.0', 1), '1.4.1-alpha.2'),
            (('1.5.0', 'v1.4.0', 2, 'v1.4.0', 1), '1.5.0-alpha.2'),
            (('1.4.0', 'v1.4.0-beta.1', 2, 'v1.4.0', 1), '1.4.1-alpha.2'),
            (('1.2.10', 'v1.2.9f-flipperclipper', 3, 'v1.2.9', 1), '1.2.10-alpha.3'),
            (('1.2.9', 'v1.2.9f-flipperclipper', 0, 'v1.2.9', 1), '1.2.9-legacy.f.flipperclipper'),
        ]
        for args, expected in cases:
            with self.subTest(args=args):
                self.assertEqual(alpha_version(*args), expected)
                self.assertIsNotNone(parse_version(expected, legacy=False))

    def test_changelog_exact_section(self):
        text = '# Changelog\n\n## Unreleased\n\n## 1.2.10-beta.1 - 2026-09-12\n\n- Fixed downloads.\n\n## 1.2.9 - 2026-08-01\n\n- Older notes.\n'
        self.assertEqual(changelog_section(text, '1.2.10-beta.1'), '- Fixed downloads.')
        for content, version in ((text, '1.2.10'), (text + text, '1.2.10-beta.1'),
                                 ('## 1.2.10 - 2026-09-12\n', '1.2.10')):
            with self.assertRaises(ValueError):
                changelog_section(content, version)

    def test_version_override_preserves_source(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'version.txt'
            path.write_text('1.2.10', encoding='utf-8')
            self.assertEqual(read_build_version(directory, '1.2.10-beta.11'), '1.2.10-beta.11')
            self.assertEqual(path.read_text(), '1.2.10')
            self.assertEqual(read_build_version(directory), '1.2.10')

    def test_alpha_reads_full_git_history_and_stable_on_other_branch(self):
        with tempfile.TemporaryDirectory() as directory:
            def git(*args):
                return subprocess.check_output(('git', '-C', directory, *args), text=True).strip()
            git('init', '-b', 'beta')
            git('config', 'user.name', 'Test')
            git('config', 'user.email', 'test@example.test')
            git('commit', '--allow-empty', '-m', 'Initial')
            git('tag', '-a', 'v1.4.0-beta.1', '-m', 'Beta')
            git('branch', 'main')
            git('commit', '--allow-empty', '-m', 'Fix')
            git('checkout', 'main')
            git('merge', '--no-ff', 'beta', '-m', 'Release')
            git('tag', 'v1.4.0')
            git('checkout', 'beta')
            git('commit', '--allow-empty', '-m', 'Next fix')
            with mock.patch('prepare_version.git', side_effect=git):
                self.assertEqual(calculate_alpha('1.4.0', 9), '1.4.1-alpha.2')

    def test_tag_placement(self):
        responses = {'v1.2.10^{commit}': 'merge', 'origin/main': 'merge', 'origin/beta': 'beta'}
        def git(*args):
            return responses[args[1]] if args[0] == 'rev-parse' else 'merge old-main beta'
        with mock.patch('prepare_version.git', side_effect=git):
            validate_placement('v1.2.10')
            responses['origin/beta'] = 'other'
            with self.assertRaises(ValueError):
                validate_placement('v1.2.10')


if __name__ == '__main__':
    unittest.main()
