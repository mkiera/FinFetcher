import unittest
from unittest import mock

from test_flipperclipper_download import app_module


def release(version, prerelease=False, installer=True):
    name = 'FinFetcher-Setup.exe' if installer else 'FinFetcher.exe'
    return {'tag_name': f'v{version}', 'prerelease': prerelease,
            'assets': [{'name': name, 'size': 100,
                        'browser_download_url': f'https://github.com/mkiera/FinFetcher/releases/download/v{version}/{name}'}]}


class UpdateVersionTests(unittest.TestCase):
    def setUp(self):
        self.manager = app_module.update_manager
        for patch in (
            mock.patch.object(self.manager, '_config', {'update_channel': 'stable'}),
            mock.patch.object(self.manager, 'get_current_version', return_value='1.2.9'),
            mock.patch.object(self.manager, '_record_check'),
            mock.patch.object(app_module.release_cache, 'get_artifacts', return_value={}),
        ):
            patch.start()
            self.addCleanup(patch.stop)

    def test_beta_selects_numeric_latest_and_stable_replaces_its_beta(self):
        self.manager._config['update_channel'] = 'prerelease'
        rows = [release('1.2.10-beta.2', True), release('1.2.10-beta.11', True)]
        with mock.patch.object(app_module.release_cache, 'get_releases', return_value=rows):
            self.assertEqual(self.manager.check_for_updates(force=True)['update']['version'], '1.2.10-beta.11')
            rows.append(release('1.2.10'))
            self.assertEqual(self.manager.check_for_updates(force=True)['update']['version'], '1.2.10')

    def test_stable_excludes_misflagged_betas_and_invalid_versions(self):
        rows = [release('1.3.0-beta.1'), release('invalid'), release('1.2.10')]
        with mock.patch.object(app_module.release_cache, 'get_releases', return_value=rows):
            self.assertEqual(self.manager.check_for_updates(force=True)['update']['version'], '1.2.10')
            body = app_module.app.test_client().get('/api/update/releases?channel=stable').get_json()
            self.assertEqual([r['version'] for r in body['releases']], ['1.2.10'])

    def test_manual_list_keeps_historical_assets_and_ignores_metadata_for_current(self):
        rows = [release('1.2.4b', True, False), release('1.2.9+build.1'),
                release('1.2.4', False, False), release('1.2.9f-flipperclipper', True)]
        with mock.patch.object(app_module.release_cache, 'get_releases', return_value=rows):
            body = app_module.app.test_client().get('/api/update/releases?channel=prerelease').get_json()
        self.assertEqual([r['version'] for r in body['releases']],
                         ['1.2.9+build.1', '1.2.9f-flipperclipper', '1.2.4', '1.2.4b'])
        self.assertTrue(body['releases'][0]['is_current'])
        self.assertEqual(body['releases'][-1]['exe_asset']['name'], 'FinFetcher.exe')

    def test_alpha_only_installs_manually(self):
        self.manager._config['update_channel'] = 'alpha'
        with mock.patch.object(app_module.release_cache, 'get_releases') as fetch:
            self.assertEqual(self.manager.check_for_updates(force=True)['reason'], 'manual_alpha')
            fetch.assert_not_called()


if __name__ == '__main__':
    unittest.main()
