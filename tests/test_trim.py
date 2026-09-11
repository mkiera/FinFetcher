import ast
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
import time
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest import mock

from test_flipperclipper_download import app_module, FakeYoutubeDL
from yt_dlp import YoutubeDL
from yt_dlp.downloader.external import FFmpegFD
import trimming


class TrimTests(unittest.TestCase):
    def options(self, precise):
        captured = {}

        class Capture(FakeYoutubeDL):
            def __init__(self, options):
                super().__init__(options)
                captured.update(options)

            def add_post_processor(self, processor, when):
                pass

        settings = dict(app_module.AppSettings.DEFAULTS, precise_trim=precise,
                        embed_thumbnail=False, embed_metadata=False)
        with tempfile.TemporaryDirectory() as directory:
            Capture.output_path = os.path.join(directory, 'clip.mp4')
            with (
                mock.patch.object(app_module.yt_dlp, 'YoutubeDL', Capture),
                mock.patch.object(app_module.settings_manager, 'get_settings', return_value=settings),
                mock.patch.object(app_module, 'get_cookie_opts', return_value={}),
                mock.patch.object(app_module.js_runtime_manager, 'any_available', return_value=True),
            ):
                response = app_module.app.test_client().post('/api/download', json={
                    'url': 'https://example.test/video', 'mode': 'video',
                    'save_path': directory, 'trim_start': '3', 'trim_end': '7',
                })
                response.get_data()
        return captured

    @unittest.skipUnless(shutil.which('ffmpeg'), 'FFmpeg required')
    def test_copy_cut_has_no_hidden_preroll(self):
        options = self.options(False)
        with tempfile.TemporaryDirectory() as directory:
            source = os.path.join(directory, 'source.mp4')
            target = os.path.join(directory, 'clip.mp4')
            subprocess.run([
                'ffmpeg', '-v', 'error', '-f', 'lavfi', '-i',
                'testsrc2=size=160x90:rate=30:duration=10',
                '-c:v', 'libx264', '-g', '150', '-keyint_min', '150',
                '-sc_threshold', '0', '-bf', '0', source,
            ], check=True, capture_output=True)
            with YoutubeDL({'quiet': True}) as ydl:
                downloader = FFmpegFD(ydl, options)
                self.assertEqual(downloader._call_downloader(target, {
                    'url': source, 'protocol': 'file', 'ext': 'mp4',
                    'section_start': 3, 'section_end': 7,
                }), 0)
            packets = json.loads(subprocess.check_output([
                'ffprobe', '-v', 'error', '-select_streams', 'v:0',
                '-show_packets', '-show_entries', 'packet=pts_time,flags',
                '-of', 'json', target,
            ]))['packets']
            self.assertGreaterEqual(float(packets[0]['pts_time']), 0)
            self.assertIn('K', packets[0]['flags'])

    def test_precise_range_sets_encoder_speed(self):
        options = self.options(True)
        arguments = options.get('external_downloader_args', {}).get('ffmpeg_o', [])
        self.assertIn('-preset', arguments)

    def test_successful_range_retry_is_not_trimmed_twice(self):
        class Retry(FakeYoutubeDL):
            attempts = 0

            def add_post_processor(self, processor, when):
                pass

            def download(self, urls):
                Retry.attempts += 1
                if Retry.attempts == 1:
                    raise RuntimeError('HTTP Error 403')
                super().download(urls)

        settings = dict(app_module.AppSettings.DEFAULTS, embed_thumbnail=False)
        with tempfile.TemporaryDirectory() as directory:
            Retry.output_path = os.path.join(directory, 'clip.mp4')
            with (
                mock.patch.object(app_module.yt_dlp, 'YoutubeDL', Retry),
                mock.patch.object(app_module.settings_manager, 'get_settings', return_value=settings),
                mock.patch.object(app_module, 'get_cookie_opts', return_value={}),
                mock.patch.object(app_module.js_runtime_manager, 'any_available', return_value=True),
                mock.patch.object(app_module, '_has_usable_output', return_value=True),
                mock.patch.object(app_module.subprocess, 'Popen') as local_trim,
            ):
                response = app_module.app.test_client().post('/api/download', json={
                    'url': 'https://example.test/video', 'mode': 'video',
                    'save_path': directory, 'trim_start': '30', 'trim_end': '34',
                })
                body = response.get_data(as_text=True)
            self.assertEqual(Retry.attempts, 2)
            self.assertIn('"status": "completed"', body, body)
            local_trim.assert_not_called()

    def test_local_trim_seeks_before_decoding(self):
        tree = ast.parse(Path(app_module.__file__).read_text(encoding='utf-8'))
        commands = [node.value for node in ast.walk(tree)
                    if isinstance(node, ast.Assign)
                    and any(isinstance(t, ast.Name) and t.id == 'ffmpeg_cmd' for t in node.targets)]
        for command in commands:
            values = [node.value for node in ast.walk(command)
                      if isinstance(node, ast.Constant) and isinstance(node.value, str)]
            if '-ss' in values and '-i' in values:
                self.assertLess(values.index('-ss'), values.index('-i'))

    @unittest.skipUnless(shutil.which('ffmpeg'), 'FFmpeg required')
    def test_real_download_pipeline_produces_moving_clips(self):
        for extension in ('mp4', 'webm'):
            with self.subTest(extension=extension):
                self.check_real_pipeline(extension)

    def check_real_pipeline(self, extension):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / f'source.{extension}'
            codec = (['-c:v', 'libx264', '-g', '150', '-keyint_min', '150',
                      '-sc_threshold', '0', '-c:a', 'aac'] if extension == 'mp4'
                     else ['-c:v', 'libvpx-vp9', '-g', '150', '-cpu-used', '5', '-c:a', 'libopus'])
            subprocess.run([
                'ffmpeg', '-v', 'error', '-f', 'lavfi', '-i',
                'testsrc2=size=320x180:rate=30:duration=10',
                '-f', 'lavfi', '-i', 'sine=frequency=440:duration=10',
            ] + codec + [str(source)], check=True, capture_output=True)
            payload = source.read_bytes()

            class Handler(BaseHTTPRequestHandler):
                def log_message(self, *args):
                    pass

                def do_GET(self):
                    start, end = 0, len(payload) - 1
                    requested = self.headers.get('Range')
                    if requested:
                        left, right = requested.removeprefix('bytes=').split('-')
                        start = int(left)
                        end = int(right) if right else end
                    self.send_response(206 if requested else 200)
                    self.send_header('Content-Type', f'video/{extension}')
                    self.send_header('Content-Length', str(end - start + 1))
                    self.send_header('Accept-Ranges', 'bytes')
                    if requested:
                        self.send_header('Content-Range', f'bytes {start}-{end}/{len(payload)}')
                    self.end_headers()
                    try:
                        self.wfile.write(payload[start:end + 1])
                    except (BrokenPipeError, ConnectionResetError):
                        pass

            server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            self.addCleanup(server.server_close)
            self.addCleanup(server.shutdown)
            for fast in (True, False):
                for precise in (True, False):
                    with self.subTest(fast=fast, precise=precise):
                        destination = Path(directory) / f'{fast}-{precise}'
                        destination.mkdir()
                        settings = dict(app_module.AppSettings.DEFAULTS, fast_trim=fast,
                                        precise_trim=precise, embed_thumbnail=False,
                                        embed_metadata=False, embed_chapters=False)
                        started = time.perf_counter()
                        with (
                            mock.patch.object(app_module.settings_manager, 'get_settings', return_value=settings),
                            mock.patch.object(app_module, 'get_cookie_opts', return_value={'quiet': True, 'noprogress': True}),
                            mock.patch.object(app_module.js_runtime_manager, 'any_available', return_value=True),
                        ):
                            response = app_module.app.test_client().post('/api/download', json={
                                'url': f'http://127.0.0.1:{server.server_port}/source.{extension}', 'mode': 'video',
                                'save_path': str(destination), 'trim_start': '3', 'trim_end': '7',
                            })
                            body = response.get_data(as_text=True)
                        self.assertIn('"status": "completed"', body, body)
                        if fast:
                            self.assertNotIn('Downloading in full', body, body)
                        output = next(destination.glob(f'*.{extension}'))
                        frames = subprocess.check_output([
                            'ffmpeg', '-v', 'error', '-i', str(output), '-t', '0.5',
                            '-map', '0:v:0', '-f', 'framemd5', '-',
                        ], text=True)
                        hashes = [line.rsplit(',', 1)[-1].strip() for line in frames.splitlines()
                                  if line and not line.startswith('#')]
                        self.assertGreater(len(set(hashes)), 10, frames)
                        info = json.loads(subprocess.check_output([
                            'ffprobe', '-v', 'error', '-show_streams', '-show_format',
                            '-of', 'json', str(output),
                        ]))
                        video = next(s for s in info['streams'] if s['codec_type'] == 'video')
                        self.assertLess(float(video.get('start_time', 0)), 0.15)
                        if precise:
                            self.assertAlmostEqual(float(info['format']['duration']), 4, delta=0.15)
                        print(f'{extension}: fast={fast}, precise={precise}: {time.perf_counter() - started:.2f}s')

            if extension == 'webm':
                return

            for action in ('overwrite', 'folder', 'cancel'):
                with self.subTest(destination_action=action):
                    destination = Path(directory) / action
                    destination.mkdir()
                    old_file = destination / 'source.mp4'
                    old_file.write_bytes(b'original clip')
                    alternate = Path(directory) / f'{action}-alternate'
                    alternate.mkdir()
                    settings.update(fast_trim=True, precise_trim=True)
                    events = []
                    prompted = False
                    with (
                        mock.patch.object(app_module.settings_manager, 'get_settings', return_value=settings),
                        mock.patch.object(app_module, 'get_cookie_opts', return_value={'quiet': True, 'noprogress': True}),
                        mock.patch.object(app_module.js_runtime_manager, 'any_available', return_value=True),
                    ):
                        client = app_module.app.test_client()
                        response = client.post('/api/download', json={
                            'url': f'http://127.0.0.1:{server.server_port}/source.mp4', 'mode': 'video',
                            'save_path': str(destination), 'trim_start': '3', 'trim_end': '7',
                        })
                        for chunk in response.response:
                            event = json.loads(chunk.decode().removeprefix('data: ').strip())
                            events.append(event)
                            if 'destination_request' in event:
                                prompted = True
                                self.assertEqual(old_file.read_bytes(), b'original clip')
                                self.assertFalse(list(destination.glob('.finfetcher-*/*.mp4')))
                                result = client.post('/api/download/destination', json={
                                    'id': event['destination_request']['id'],
                                    'action': action, 'path': str(alternate),
                                })
                                self.assertEqual(result.status_code, 200)
                        response.close()
                    self.assertTrue(prompted, events)
                    statuses = [e['status'] for e in events if 'status' in e]
                    self.assertEqual(statuses[-1], 'cancelled' if action == 'cancel' else 'completed', events)
                    if action != 'overwrite':
                        self.assertEqual(old_file.read_bytes(), b'original clip')
                    if action == 'folder':
                        self.assertGreater((alternate / 'source.mp4').stat().st_size, 1000)
                    elif action == 'overwrite':
                        self.assertGreater(old_file.stat().st_size, 1000)

            original_args = trimming.video_encode_args

            def broken_hardware(extension, encoder='libx264'):
                arguments = original_args(extension, encoder)
                if encoder != 'libx264':
                    arguments[1] = 'missing_hardware_encoder'
                return arguments

            for fast in (True, False):
                with self.subTest(hardware_failure=True, fast=fast):
                    destination = Path(directory) / f'fallback-{fast}'
                    settings.update(fast_trim=fast, precise_trim=True)
                    with (
                        mock.patch.object(app_module.settings_manager, 'get_settings', return_value=settings),
                        mock.patch.object(app_module, 'get_cookie_opts', return_value={'quiet': True, 'noprogress': True}),
                        mock.patch.object(app_module.js_runtime_manager, 'any_available', return_value=True),
                        mock.patch.object(trimming, 'select_encoder', return_value='h264_nvenc'),
                        mock.patch.object(app_module, 'select_encoder', return_value='h264_nvenc'),
                        mock.patch.object(trimming, 'video_encode_args', side_effect=broken_hardware),
                        mock.patch.object(app_module, 'video_encode_args', side_effect=broken_hardware),
                    ):
                        response = app_module.app.test_client().post('/api/download', json={
                            'url': f'http://127.0.0.1:{server.server_port}/source.mp4', 'mode': 'video',
                            'save_path': str(destination), 'trim_start': '3', 'trim_end': '7',
                        })
                        body = response.get_data(as_text=True)
                    self.assertIn('Hardware trim failed. Retrying with software encoding.', body, body)
                    self.assertIn('"status": "completed"', body, body)
                    output = next(destination.glob('*.mp4'))
                    duration = subprocess.check_output([
                        'ffprobe', '-v', 'error', '-show_entries', 'format=duration',
                        '-of', 'default=noprint_wrappers=1:nokey=1', str(output),
                    ], text=True)
                    self.assertAlmostEqual(float(duration), 4, delta=0.15)


if __name__ == '__main__':
    unittest.main()
