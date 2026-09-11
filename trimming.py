from yt_dlp.postprocessor import PostProcessor
from functools import lru_cache
import os
import subprocess


@lru_cache(maxsize=4)
def select_encoder(ffmpeg):
    for encoder in ('h264_nvenc', 'h264_qsv', 'h264_amf'):
        try:
            result = subprocess.run(
                [ffmpeg, '-v', 'error', '-f', 'lavfi', '-i', 'nullsrc=s=256x144',
                 '-frames:v', '1', '-c:v', encoder, '-f', 'null', '-'],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, timeout=5,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0,
            )
            if result.returncode == 0:
                return encoder
        except (OSError, subprocess.TimeoutExpired):
            continue
    return 'libx264'


def video_encode_args(extension, encoder='libx264'):
    if extension.lstrip('.').lower() == 'webm':
        return ['-c:v', 'libvpx-vp9', '-deadline', 'good', '-cpu-used', '5',
                '-row-mt', '1', '-crf', '31', '-b:v', '0',
                '-c:a', 'libopus', '-b:a', '128k']
    quality = {
        'h264_nvenc': ['-rc', 'vbr', '-cq', '22', '-b:v', '0'],
        'h264_qsv': ['-global_quality', '22'],
        'h264_amf': ['-rc', 'cqp', '-qp_i', '22', '-qp_p', '22'],
        'libx264': ['-preset', 'veryfast', '-crf', '22'],
    }[encoder]
    return ['-c:v', encoder] + quality + ['-c:a', 'aac', '-b:a', '192k']


def configure_range_trim(options, mode, precise, extension, encoder='libx264'):
    arguments = []
    if precise and mode == 'video':
        arguments.extend(video_encode_args(extension, encoder))
    if not precise:
        arguments.extend(['-avoid_negative_ts', 'make_zero'])
    options.setdefault('external_downloader_args', {})['ffmpeg_o'] = arguments


class RangeTrimPP(PostProcessor):
    def __init__(self, downloader, mode, precise, ffmpeg):
        super().__init__(downloader)
        self.mode = mode
        self.precise = precise
        self.ffmpeg = ffmpeg

    def run(self, info):
        # A single progressive format keeps its own container instead of merge_output_format.
        options = self._downloader.params
        encoder = 'libx264'
        if (self.mode == 'video' and self.precise and info['ext'] != 'webm'
                and not options.get('_trim_software')):
            encoder = select_encoder(self.ffmpeg)
        options['_trim_encoder'] = encoder
        configure_range_trim(options, self.mode, self.precise, info['ext'], encoder)
        return [], info
