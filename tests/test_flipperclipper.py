import os
import tempfile
import unittest
from unittest import mock

import flipperclipper


class FindFlipperClipperTests(unittest.TestCase):
    @mock.patch.object(flipperclipper, '_registered_flipperclipper')
    def test_prefers_registered_install(self, registered):
        registered.return_value = r'D:\Apps\FlipperClipper\FlipperClipper.exe'

        with mock.patch.object(flipperclipper.os, 'name', 'nt'):
            result = flipperclipper.find_flipperclipper()

        self.assertEqual(
            result, os.path.abspath(r'D:\Apps\FlipperClipper\FlipperClipper.exe')
        )

    @mock.patch.object(flipperclipper, '_registered_flipperclipper', return_value=None)
    @mock.patch.object(flipperclipper.os.path, 'isfile')
    @mock.patch.object(flipperclipper.shutil, 'which')
    def test_prefers_executable_on_path(self, which, isfile, _registered):
        which.return_value = r'C:\Tools\FlipperClipper.exe'
        isfile.return_value = True

        with mock.patch.object(flipperclipper.os, 'name', 'nt'):
            result = flipperclipper.find_flipperclipper()

        self.assertEqual(result, os.path.abspath(r'C:\Tools\FlipperClipper.exe'))

    @mock.patch.object(flipperclipper, '_registered_flipperclipper', return_value=None)
    @mock.patch.object(flipperclipper.shutil, 'which', return_value=None)
    def test_finds_standard_per_user_install(self, _which, _registered):
        with tempfile.TemporaryDirectory() as local_app_data:
            install_dir = os.path.join(local_app_data, 'Programs', 'FlipperClipper')
            os.makedirs(install_dir)
            executable = os.path.join(install_dir, 'FlipperClipper.exe')
            with open(executable, 'wb'):
                pass

            with mock.patch.object(flipperclipper.os, 'name', 'nt'), mock.patch.dict(
                os.environ,
                {'LOCALAPPDATA': local_app_data, 'ProgramFiles': '', 'ProgramFiles(x86)': ''},
            ):
                result = flipperclipper.find_flipperclipper()

        self.assertEqual(result, os.path.abspath(executable))


class OpenInFlipperClipperTests(unittest.TestCase):
    @mock.patch.object(flipperclipper.subprocess, 'Popen')
    def test_passes_downloaded_video_as_an_argument(self, popen):
        with tempfile.NamedTemporaryFile(suffix='.mp4') as video:
            executable = os.path.abspath('FlipperClipper.exe')
            opened, error = flipperclipper.open_in_flipperclipper(video.name, executable)

        self.assertTrue(opened)
        self.assertIsNone(error)
        popen.assert_called_once()
        self.assertEqual(popen.call_args.args[0], [executable, os.path.abspath(video.name)])

    @mock.patch.object(flipperclipper.subprocess, 'Popen')
    def test_rejects_a_missing_video(self, popen):
        opened, error = flipperclipper.open_in_flipperclipper(
            'missing-video.mp4', os.path.abspath('FlipperClipper.exe')
        )

        self.assertFalse(opened)
        self.assertIn('not found', error)
        popen.assert_not_called()

    @mock.patch.object(flipperclipper.subprocess, 'Popen')
    def test_rejects_a_missing_video_path(self, popen):
        opened, error = flipperclipper.open_in_flipperclipper(
            None, os.path.abspath('FlipperClipper.exe')
        )

        self.assertFalse(opened)
        self.assertIn('without a video path', error)
        popen.assert_not_called()


if __name__ == '__main__':
    unittest.main()
