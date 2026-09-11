import os
import tempfile
import unittest
from importlib.machinery import SourceFileLoader
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
app_module = SourceFileLoader(
    'finfetcher_flipperclipper_test', str(ROOT / 'main.pyw')
).load_module()


class FakeYoutubeDL:
    output_path = None

    def __init__(self, options):
        self.options = options
        self.params = options

    def __enter__(self):
        return self

    def add_post_processor(self, processor, when):
        pass

    def __exit__(self, *_args):
        return False

    def download(self, _urls):
        template = self.options['outtmpl']
        if isinstance(template, dict):
            template = template['default']
        output_path = os.path.join(os.path.dirname(template), os.path.basename(self.output_path))
        with open(output_path, 'wb') as video:
            video.write(b'video')
        for hook in self.options['progress_hooks']:
            hook({'status': 'finished', 'filename': output_path})
        for hook in self.options['postprocessor_hooks']:
            hook({
                'status': 'finished',
                'info_dict': {'filepath': output_path},
            })


class FlipperClipperDownloadTests(unittest.TestCase):
    def test_successful_download_opens_flipperclipper_when_requested(self):
        with tempfile.TemporaryDirectory() as save_path:
            FakeYoutubeDL.output_path = os.path.join(save_path, 'clip.mp4')
            with (
                mock.patch.object(app_module.yt_dlp, 'YoutubeDL', FakeYoutubeDL),
                mock.patch.object(
                    app_module.js_runtime_manager, 'any_available', return_value=True
                ),
                mock.patch.object(
                    app_module, 'open_in_flipperclipper', return_value=(True, None)
                ) as open_video,
            ):
                response = app_module.app.test_client().post('/api/download', json={
                    'url': 'https://example.test/video',
                    'mode': 'video',
                    'type': 'single',
                    'save_path': save_path,
                    'pass_to_flipperclipper': True,
                })
                body = response.get_data(as_text=True)

        self.assertEqual(response.status_code, 200)
        self.assertIn('Opened the video in FlipperClipper.', body)
        self.assertIn('"status": "completed"', body)
        open_video.assert_called_once_with(FakeYoutubeDL.output_path)


if __name__ == '__main__':
    unittest.main()
