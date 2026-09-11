from pathlib import Path
import tempfile
import unittest
from unittest import mock

from download_output import DownloadOutput


class DownloadOutputTests(unittest.TestCase):
    def test_declining_keeps_the_existing_clip(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'clip.mp4'
            target.write_bytes(b'old clip')
            output = DownloadOutput(directory)
            self.addCleanup(output.cleanup)
            (Path(output.directory) / 'clip.mp4').write_bytes(b'new clip')
            confirm = mock.Mock(return_value={'action': 'cancel'})
            self.assertFalse(output.prepare(['clip.mp4'], confirm, lambda: False))
            self.assertEqual(target.read_bytes(), b'old clip')
            confirm.assert_called_once_with([str(target)])

    def test_accepting_replaces_the_existing_clip(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'clip.mp4'
            target.write_bytes(b'old clip')
            output = DownloadOutput(directory)
            self.addCleanup(output.cleanup)
            source = Path(output.directory) / 'clip.mp4'
            source.write_bytes(b'new clip')
            output.prepare(['clip.mp4'], lambda paths: {'action': 'overwrite'}, lambda: False)
            published = output.publish(lambda: False)
            self.assertEqual(target.read_bytes(), b'new clip')
            self.assertEqual(published[str(source)], str(target))

    def test_cancelling_during_confirmation_preserves_the_old_clip(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'clip.mp4'
            target.write_bytes(b'old clip')
            output = DownloadOutput(directory)
            self.addCleanup(output.cleanup)
            (Path(output.directory) / 'clip.mp4').write_bytes(b'new clip')
            self.assertIsNone(output.publish(lambda: True))
            self.assertEqual(target.read_bytes(), b'old clip')

    def test_new_filename_saves_without_asking(self):
        with tempfile.TemporaryDirectory() as directory:
            output = DownloadOutput(directory)
            self.addCleanup(output.cleanup)
            (Path(output.directory) / 'clip.mp4').write_bytes(b'new clip')
            confirm = mock.Mock()
            output.prepare(['clip.mp4'], confirm, lambda: False)
            output.publish(lambda: False)
            self.assertEqual((Path(directory) / 'clip.mp4').read_bytes(), b'new clip')
            confirm.assert_not_called()

    def test_choosing_another_folder_keeps_the_original(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'clip.mp4'
            target.write_bytes(b'old clip')
            elsewhere = Path(directory) / 'elsewhere'
            elsewhere.mkdir()
            output = DownloadOutput(directory)
            self.addCleanup(output.cleanup)
            source = Path(output.directory) / 'clip.mp4'
            source.write_bytes(b'new clip')
            self.assertTrue(output.prepare(['clip.mp4'], lambda paths: {
                'action': 'folder', 'path': str(elsewhere),
            }, lambda: False))
            output.publish(lambda: False)
            self.assertEqual(target.read_bytes(), b'old clip')
            self.assertEqual((elsewhere / 'clip.mp4').read_bytes(), b'new clip')
