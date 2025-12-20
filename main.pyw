"""
FinFetcher 🦭
A friendly video & music downloader desktop application.
Built with Flask + PyWebView for native desktop experience.
"""


import os
import sys
import json
import subprocess
import webview
import threading
import queue
import time
import zipfile
import tempfile
import shutil
import yt_dlp
from flask import Flask, request, jsonify, send_from_directory, Response
from urllib.request import urlopen, Request
from urllib.error import URLError


class FFmpegManager:
    """Manages ffmpeg installation and detection."""
    
    # FFmpeg download URL (gyan.dev essentials build - smaller ~30MB)
    FFMPEG_URL = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
    
    def __init__(self):
        self._custom_path = None
        self._config_file = os.path.join(self.get_app_data_dir(), 'config.json')
        self._load_config()
    
    @staticmethod
    def get_app_data_dir():
        """Get the FinFetcher app data directory."""
        if os.name == 'nt':
            base = os.environ.get('APPDATA', os.path.expanduser('~'))
        else:
            base = os.path.expanduser('~/.config')
        app_dir = os.path.join(base, 'FinFetcher')
        os.makedirs(app_dir, exist_ok=True)
        return app_dir
    
    def get_ffmpeg_dir(self):
        """Get the directory containing ffmpeg binaries."""
        if self._custom_path and os.path.exists(self._custom_path):
            return self._custom_path
        return os.path.join(self.get_app_data_dir(), 'ffmpeg')
    
    def get_ffmpeg_path(self):
        """Get full path to ffmpeg executable."""
        ffmpeg_name = 'ffmpeg.exe' if os.name == 'nt' else 'ffmpeg'
        return os.path.join(self.get_ffmpeg_dir(), ffmpeg_name)
    
    def get_ffprobe_path(self):
        """Get full path to ffprobe executable."""
        ffprobe_name = 'ffprobe.exe' if os.name == 'nt' else 'ffprobe'
        return os.path.join(self.get_ffmpeg_dir(), ffprobe_name)
    
    def is_installed(self):
        """Check if ffmpeg is available."""
        ffmpeg_path = self.get_ffmpeg_path()
        return os.path.exists(ffmpeg_path)
    
    def set_custom_path(self, path):
        """Set a custom ffmpeg directory path."""
        ffmpeg_exe = os.path.join(path, 'ffmpeg.exe' if os.name == 'nt' else 'ffmpeg')
        if os.path.exists(ffmpeg_exe):
            self._custom_path = path
            self._save_config()
            return True
        return False
    
    def _load_config(self):
        """Load saved configuration."""
        try:
            if os.path.exists(self._config_file):
                with open(self._config_file, 'r') as f:
                    config = json.load(f)
                    self._custom_path = config.get('ffmpeg_path')
        except Exception:
            pass
    
    def _save_config(self):
        """Save configuration to disk."""
        try:
            config = {'ffmpeg_path': self._custom_path}
            with open(self._config_file, 'w') as f:
                json.dump(config, f)
        except Exception:
            pass
    
    def download_ffmpeg(self, progress_callback=None):
        """
        Download and install ffmpeg.
        progress_callback(percent, status_text) is called with progress updates.
        Returns True on success, False on failure.
        """
        try:
            if progress_callback:
                progress_callback(0, "Connecting to download server...")
            
            # Create request with User-Agent
            req = Request(self.FFMPEG_URL, headers={'User-Agent': 'FinFetcher/1.0'})
            
            # Download to temp file
            temp_dir = tempfile.mkdtemp()
            zip_path = os.path.join(temp_dir, 'ffmpeg.zip')
            
            try:
                with urlopen(req, timeout=60) as response:
                    total_size = int(response.headers.get('Content-Length', 0))
                    downloaded = 0
                    chunk_size = 1024 * 64  # 64KB chunks
                    
                    with open(zip_path, 'wb') as f:
                        while True:
                            chunk = response.read(chunk_size)
                            if not chunk:
                                break
                            f.write(chunk)
                            downloaded += len(chunk)
                            
                            if total_size > 0 and progress_callback:
                                percent = int((downloaded / total_size) * 70)  # 0-70% for download
                                size_mb = downloaded / (1024 * 1024)
                                total_mb = total_size / (1024 * 1024)
                                progress_callback(percent, f"Downloading... {size_mb:.1f}/{total_mb:.1f} MB")
                
                if progress_callback:
                    progress_callback(70, "Extracting files...")
                
                # Extract ffmpeg
                ffmpeg_dir = os.path.join(self.get_app_data_dir(), 'ffmpeg')
                os.makedirs(ffmpeg_dir, exist_ok=True)
                
                with zipfile.ZipFile(zip_path, 'r') as zf:
                    # Find the bin folder in the archive
                    bin_files = [n for n in zf.namelist() if '/bin/' in n and n.endswith('.exe')]
                    total_files = len(bin_files)
                    
                    for i, name in enumerate(bin_files):
                        # Extract just the filename, not the path
                        filename = os.path.basename(name)
                        if filename:
                            if progress_callback:
                                progress_callback(70 + int((i / max(total_files, 1)) * 25), f"Extracting {filename}...")
                            
                            # Extract to ffmpeg dir
                            with zf.open(name) as src, open(os.path.join(ffmpeg_dir, filename), 'wb') as dst:
                                dst.write(src.read())
                
                if progress_callback:
                    progress_callback(95, "Verifying installation...")
                
                # Verify
                if self.is_installed():
                    if progress_callback:
                        progress_callback(100, "Installation complete!")
                    return True
                else:
                    if progress_callback:
                        progress_callback(0, "Error: FFmpeg not found after extraction")
                    return False
                    
            finally:
                # Cleanup temp files
                try:
                    shutil.rmtree(temp_dir)
                except Exception:
                    pass
                    
        except URLError as e:
            if progress_callback:
                progress_callback(0, f"Download failed: {str(e)}")
            return False
        except Exception as e:
            if progress_callback:
                progress_callback(0, f"Error: {str(e)}")
            return False


# Global FFmpeg manager instance
ffmpeg_manager = FFmpegManager()


def get_ffmpeg_path():
    """Get the path to ffmpeg executable."""
    return ffmpeg_manager.get_ffmpeg_path()


def get_ffprobe_path():
    """Get the path to ffprobe executable."""
    return ffmpeg_manager.get_ffprobe_path()


def get_ffmpeg_dir():
    """Get the directory containing ffmpeg binaries."""
    if ffmpeg_manager.is_installed():
        return ffmpeg_manager.get_ffmpeg_dir()
    return None

class Api:
    """PyWebView API for native dialog access."""
    def select_folder(self):
        folder = webview.windows[0].create_file_dialog(webview.FOLDER_DIALOG)
        return folder[0] if folder else None

app = Flask(__name__, static_folder='.')

@app.route('/')
def index():
    return send_from_directory('.', 'index.html')

@app.route('/<path:path>')
def serve_static(path):
    return send_from_directory('.', path)


# ============ Setup API Endpoints ============

@app.route('/api/setup/check', methods=['GET'])
def setup_check():
    """Check if ffmpeg is installed."""
    return jsonify({
        'installed': ffmpeg_manager.is_installed(),
        'path': ffmpeg_manager.get_ffmpeg_dir() if ffmpeg_manager.is_installed() else None
    })


@app.route('/api/setup/install', methods=['GET'])
def setup_install():
    """Download and install ffmpeg with SSE progress updates."""
    def generate():
        def progress_callback(percent, status):
            yield f"data: {json.dumps({'percent': percent, 'status': status})}\n\n"
        
        # Use a list to capture the result from the callback
        result = [False]
        last_update = [None]
        
        def wrapped_callback(percent, status):
            last_update[0] = (percent, status)
        
        # Run download in a way that yields progress
        try:
            # We need to yield progress updates as they happen
            # So we'll run the download in chunks and yield
            yield f"data: {json.dumps({'percent': 0, 'status': 'Starting download...'})}\n\n"
            
            success = ffmpeg_manager.download_ffmpeg(
                progress_callback=lambda p, s: last_update.__setitem__(0, (p, s))
            )
            
            # Since we can't easily yield from the callback, we'll do a simpler approach
            # Just run the download and report result
            if success:
                yield f"data: {json.dumps({'percent': 100, 'status': 'Installation complete!', 'success': True})}\n\n"
            else:
                yield f"data: {json.dumps({'percent': 0, 'status': 'Installation failed', 'success': False})}\n\n"
        except Exception as e:
            yield f"data: {json.dumps({'percent': 0, 'status': f'Error: {str(e)}', 'success': False})}\n\n"
    
    return Response(generate(), mimetype='text/event-stream')


@app.route('/api/setup/install-sync', methods=['POST'])
def setup_install_sync():
    """Synchronous install endpoint that streams progress."""
    def generate():
        try:
            yield f"data: {json.dumps({'percent': 0, 'status': 'Connecting to download server...'})}\n\n"
            
            # We'll reimplement a simpler download here for streaming
            from urllib.request import urlopen, Request
            import zipfile
            import tempfile
            import shutil
            
            req = Request(FFmpegManager.FFMPEG_URL, headers={'User-Agent': 'FinFetcher/1.0'})
            temp_dir = tempfile.mkdtemp()
            zip_path = os.path.join(temp_dir, 'ffmpeg.zip')
            
            try:
                with urlopen(req, timeout=120) as response:
                    total_size = int(response.headers.get('Content-Length', 0))
                    downloaded = 0
                    chunk_size = 1024 * 64
                    
                    with open(zip_path, 'wb') as f:
                        while True:
                            chunk = response.read(chunk_size)
                            if not chunk:
                                break
                            f.write(chunk)
                            downloaded += len(chunk)
                            
                            if total_size > 0:
                                percent = int((downloaded / total_size) * 70)
                                size_mb = downloaded / (1024 * 1024)
                                total_mb = total_size / (1024 * 1024)
                                yield f"data: {json.dumps({'percent': percent, 'status': f'Downloading... {size_mb:.1f}/{total_mb:.1f} MB'})}\n\n"
                
                yield f"data: {json.dumps({'percent': 70, 'status': 'Extracting files...'})}\n\n"
                
                ffmpeg_dir = os.path.join(ffmpeg_manager.get_app_data_dir(), 'ffmpeg')
                os.makedirs(ffmpeg_dir, exist_ok=True)
                
                with zipfile.ZipFile(zip_path, 'r') as zf:
                    bin_files = [n for n in zf.namelist() if '/bin/' in n and n.endswith('.exe')]
                    total_files = len(bin_files)
                    
                    for i, name in enumerate(bin_files):
                        filename = os.path.basename(name)
                        if filename:
                            percent = 70 + int((i / max(total_files, 1)) * 25)
                            yield f"data: {json.dumps({'percent': percent, 'status': f'Extracting {filename}...'})}\n\n"
                            
                            with zf.open(name) as src, open(os.path.join(ffmpeg_dir, filename), 'wb') as dst:
                                dst.write(src.read())
                
                yield f"data: {json.dumps({'percent': 95, 'status': 'Verifying installation...'})}\n\n"
                
                if ffmpeg_manager.is_installed():
                    yield f"data: {json.dumps({'percent': 100, 'status': 'Installation complete!', 'success': True})}\n\n"
                else:
                    yield f"data: {json.dumps({'percent': 0, 'status': 'Error: FFmpeg not found after extraction', 'success': False})}\n\n"
                    
            finally:
                try:
                    shutil.rmtree(temp_dir)
                except:
                    pass
                    
        except Exception as e:
            yield f"data: {json.dumps({'percent': 0, 'status': f'Error: {str(e)}', 'success': False})}\n\n"
    
    return Response(generate(), mimetype='text/event-stream')


@app.route('/api/setup/browse', methods=['POST'])
def setup_browse():
    """Set a custom ffmpeg path."""
    data = request.json
    path = data.get('path')
    
    if not path:
        return jsonify({'success': False, 'error': 'No path provided'})
    
    if ffmpeg_manager.set_custom_path(path):
        return jsonify({'success': True, 'path': path})
    else:
        return jsonify({'success': False, 'error': 'FFmpeg not found in the selected folder'})


@app.route('/api/setup/exit', methods=['POST'])
def setup_exit():
    """Exit the application."""
    def shutdown():
        time.sleep(0.5)
        os._exit(0)
    threading.Thread(target=shutdown, daemon=True).start()
    return jsonify({'success': True})


def get_video_info(url, flat=True):
    """Fetch video metadata using yt-dlp Python API."""
    ydl_opts = {
        'quiet': True,
        'no_warnings': True,
        'extract_flat': flat,
    }
    try:
        with yt_dlp.YoutubeDL(ydl_opts) as ydl:
            return ydl.extract_info(url, download=False)
    except Exception as e:
        raise Exception(str(e))


def estimate_size(formats, quality='max'):
    """Estimate download size from format info."""
    if not formats:
        return None
    
    # Try to find best matching format
    best_video = None
    best_audio = None
    
    for f in formats:
        if f.get('vcodec') and f.get('vcodec') != 'none':
            if not best_video or (f.get('height', 0) or 0) > (best_video.get('height', 0) or 0):
                best_video = f
        if f.get('acodec') and f.get('acodec') != 'none' and not f.get('vcodec'):
            if not best_audio or (f.get('abr', 0) or 0) > (best_audio.get('abr', 0) or 0):
                best_audio = f
    
    total = 0
    if best_video and best_video.get('filesize'):
        total += best_video['filesize']
    if best_audio and best_audio.get('filesize'):
        total += best_audio['filesize']
    
    return total if total > 0 else None


def format_size(bytes_size):
    """Format bytes to human readable string."""
    if not bytes_size:
        return "Unknown"
    if bytes_size < 1024 * 1024:
        return f"{bytes_size / 1024:.1f} KB"
    elif bytes_size < 1024 * 1024 * 1024:
        return f"{bytes_size / (1024 * 1024):.1f} MB"
    else:
        return f"{bytes_size / (1024 * 1024 * 1024):.2f} GB"


@app.route('/api/info', methods=['POST'])
def get_info():
    """API endpoint to fetch video/playlist metadata with detailed info."""
    data = request.json
    url = data.get('url')
    if not url:
        return jsonify({'error': 'No URL provided'}), 400
    
    try:
        info = get_video_info(url, flat=True)
        is_playlist = 'entries' in info or info.get('_type') == 'playlist'
        
        # Base response
        response = {
            'title': info.get('title', 'Unknown Title'),
            'duration': info.get('duration', 0),
            'thumbnail': info.get('thumbnail', ''),
            'is_playlist': is_playlist,
            'formats': info.get('formats', []),
        }
        
        # Estimate size for single video
        if not is_playlist:
            size = estimate_size(info.get('formats', []))
            response['size'] = size
            response['size_formatted'] = format_size(size)
            response['entries_count'] = 1
        else:
            # For playlists, return entry info
            entries = info.get('entries', [])
            response['entries_count'] = len(entries)
            response['entries'] = []
            total_size = 0
            
            for entry in entries[:50]:  # Limit to 50 for performance
                entry_info = {
                    'id': entry.get('id', ''),
                    'title': entry.get('title', 'Unknown'),
                    'duration': entry.get('duration', 0),
                    'thumbnail': entry.get('thumbnail', ''),
                }
                response['entries'].append(entry_info)
            
            response['size'] = None
            response['size_formatted'] = "Varies per video"
        
        return jsonify(response)
    except Exception as e:
        return jsonify({'error': str(e)}), 500


@app.route('/api/debug', methods=['GET'])
def get_debug_info():
    """API endpoint for debugging - returns system info and dependency versions."""
    import platform
    import sys
    
    # Hide console window on Windows
    startupinfo = None
    creationflags = 0
    if os.name == 'nt':
        startupinfo = subprocess.STARTUPINFO()
        startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        creationflags = subprocess.CREATE_NO_WINDOW
    
    debug_info = {
        'system': {
            'os': platform.system(),
            'os_version': platform.version(),
            'platform': platform.platform(),
            'python_version': sys.version,
            'python_executable': sys.executable,
        },
        'dependencies': {},
        'paths': {
            'cwd': os.getcwd(),
            'downloads': os.path.join(os.path.expanduser("~"), "Downloads"),
        }
    }
    
    # Check yt-dlp
    try:
        debug_info['dependencies']['yt-dlp'] = yt_dlp.version.__version__
    except Exception as e:
        debug_info['dependencies']['yt-dlp'] = f"Error: {str(e)}"
    
    # Check ffmpeg
    ffmpeg_exe = get_ffmpeg_path()
    try:
        result = subprocess.run([ffmpeg_exe, '-version'], capture_output=True, text=True,
                              startupinfo=startupinfo, creationflags=creationflags, timeout=10)
        if result.returncode == 0:
            first_line = result.stdout.split('\n')[0] if result.stdout else 'Unknown'
            debug_info['dependencies']['ffmpeg'] = first_line
        else:
            debug_info['dependencies']['ffmpeg'] = f"Error: {result.stderr}"
    except FileNotFoundError:
        debug_info['dependencies']['ffmpeg'] = "NOT FOUND - ffmpeg is not installed or not in PATH"
    except Exception as e:
        debug_info['dependencies']['ffmpeg'] = f"Error: {str(e)}"
    
    return jsonify(debug_info)


@app.route('/api/debug/test', methods=['POST'])
def run_debug_test():
    """Run a diagnostic test with yt-dlp Python API."""
    data = request.json
    test_url = data.get('url', 'https://www.youtube.com/watch?v=dQw4w9WgXcQ')  # Default test video
    
    try:
        ydl_opts = {'quiet': True}
        with yt_dlp.YoutubeDL(ydl_opts) as ydl:
            info = ydl.extract_info(test_url, download=False)
            return jsonify({
                'success': True,
                'message': 'yt-dlp can fetch video info successfully!',
                'title': info.get('title', 'Unknown')
            })
    except Exception as e:
        return jsonify({
            'success': False,
            'message': 'Exception',
            'error': str(e)
        })


@app.route('/api/download', methods=['POST'])
def download():
    """API endpoint to initiate download with yt-dlp Python API."""
    data = request.json
    url = data.get('url')
    mode = data.get('mode', 'video')
    download_type = data.get('type', 'single')
    save_path = data.get('save_path')
    log_to_file = data.get('log_to_file', False)
    quality = data.get('quality', 'max')
    trim_start = data.get('trim_start')
    trim_end = data.get('trim_end')
    
    if not url:
        return jsonify({'error': 'No URL provided'}), 400

    # Set save path (default to Downloads folder)
    if not save_path:
        save_path = os.path.join(os.path.expanduser("~"), "Downloads")
    if not os.path.exists(save_path):
        os.makedirs(save_path)

    # Output template
    output_template = '%(title)s.%(ext)s' if mode == 'video' else '%(artist)s - %(title)s.%(ext)s'
    
    # Configure yt-dlp options
    ffmpeg_dir = get_ffmpeg_dir()
    ydl_opts = {
        'outtmpl': os.path.join(save_path, output_template),
        'updatetime': False,
        'noplaylist': True if download_type == 'single' else False,
    }
    
    # Set ffmpeg location if bundled
    if ffmpeg_dir:
        ydl_opts['ffmpeg_location'] = ffmpeg_dir

    # Mode-specific options
    if mode == 'audio':
        ydl_opts.update({
            'format': 'bestaudio/best',
            'postprocessors': [{
                'key': 'FFmpegExtractAudio',
                'preferredcodec': 'mp3',
                'preferredquality': '0',
            }],
        })
    else:
        # Video format selection with quality preference
        format_spec = 'bestvideo+bestaudio/best'
        if quality == '1080p':
            format_spec = 'bestvideo[height<=1080]+bestaudio/best[height<=1080]/best'
        elif quality != 'max' and quality.endswith('p'):
            try:
                h = int(quality[:-1])
                format_spec = f'bestvideo[height<={h}]+bestaudio/best[height<={h}]/best'
            except:
                pass
        
        ydl_opts.update({
            'format': format_spec,
            'merge_output_format': 'mp4',
        })

    # Queues for communicating with thread
    msg_queue = queue.Queue()
    result_queue = queue.Queue()
    
    def progress_hook(d):
        """Callback for yt-dlp progress."""
        if d['status'] == 'downloading':
            try:
                percent = d.get('_percent_str', '?').strip()
                speed = d.get('_speed_str', '?').strip()
                eta = d.get('_eta_str', '?').strip()
                total = d.get('_total_bytes_str', d.get('_total_bytes_estimate_str', '?')).strip()
                msg = f"[download] {percent} of {total} at {speed} ETA {eta}"
                msg_queue.put({'log': msg})
            except:
                pass
        elif d['status'] == 'finished':
            filename = d.get('filename', 'Unknown')
            msg_queue.put({'log': f"[download] Destination: {filename}"})
            msg_queue.put({'log': "[download] Download completed processing"})
            result_queue.put({'final_file': filename})

    ydl_opts['progress_hooks'] = [progress_hook]
    
    def postprocessor_hook(d):
        """Callback for yt-dlp post-processing (e.g., audio conversion)."""
        if d['status'] == 'finished':
            # Capture the final filepath after post-processing
            final_path = d.get('info_dict', {}).get('filepath')
            if final_path:
                msg_queue.put({'log': f"[postprocess] Final file: {final_path}"})
                result_queue.put({'final_file': final_path})
    
    ydl_opts['postprocessor_hooks'] = [postprocessor_hook]

    def run_download_thread():
        try:
            with yt_dlp.YoutubeDL(ydl_opts) as ydl:
                ydl.download([url])
            result_queue.put({'success': True})
        except Exception as e:
            result_queue.put({'success': False, 'error': str(e)})

    try:
        def generate():
            # Start download thread
            t = threading.Thread(target=run_download_thread, daemon=True)
            t.start()
            
            final_file = None
            download_success = False

            # Monitor progress
            while True:
                # 1. Yield any logs from queue
                try:
                    while True:
                        msg = msg_queue.get_nowait()
                        yield f"data: {json.dumps(msg)}\n\n"
                        
                        # Optional: log to file
                        if log_to_file:
                            try:
                                log_path = os.path.join(save_path, "download_log.txt")
                                with open(log_path, "a", encoding="utf-8") as f:
                                    f.write(msg.get('log', '') + "\n")
                            except:
                                pass
                except queue.Empty:
                    pass

                # 2. Check if thread finished
                if not t.is_alive() and msg_queue.empty():
                    break
                
                # 3. Check for specific result updates (filename, success)
                try:
                    while True:
                        res = result_queue.get_nowait()
                        if 'final_file' in res:
                            final_file = res['final_file']
                        if 'success' in res:
                            download_success = res['success']
                            if not res['success']:
                                yield f"data: {json.dumps({'error': res.get('error')})}\n\n"
                except queue.Empty:
                    pass
                
                # Avoid busy wait
                time.sleep(0.1)

            # Ensure thread is joined
            t.join(timeout=1)
            
            # Final drain of result_queue to capture final_file and success status
            # This fixes a race condition where results weren't captured before thread exit check
            try:
                while True:
                    res = result_queue.get_nowait()
                    if 'final_file' in res:
                        final_file = res['final_file']
                    if 'success' in res:
                        download_success = res['success']
                        if not res['success']:
                            yield f"data: {json.dumps({'error': res.get('error')})}\n\n"
            except queue.Empty:
                pass
            
            # Post-download trimming (if requested)
            if download_success and trim_start and trim_end and final_file:
                try:
                    yield f"data: {json.dumps({'log': f'> [FinFetcher] Trimming video from {trim_start} to {trim_end}...'})}\n\n"
                    
                    base, ext = os.path.splitext(final_file)
                    trimmed_file = f"{base}_trimmed{ext}"
                    
                    # Hide console window on Windows
                    startupinfo = None
                    creationflags = 0
                    if os.name == 'nt':
                        startupinfo = subprocess.STARTUPINFO()
                        startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
                        creationflags = subprocess.CREATE_NO_WINDOW

                    # FFmpeg precise trim
                    ffmpeg_exe = get_ffmpeg_path()
                    ffmpeg_cmd = [
                        ffmpeg_exe, '-y',
                        '-i', final_file,
                        '-ss', trim_start,
                        '-to', trim_end,
                        '-c:v', 'libx264', '-preset', 'fast', '-crf', '22',
                        '-c:a', 'aac', '-b:a', '192k',
                        '-strict', 'experimental',
                        trimmed_file
                    ]
                    
                    environ = os.environ.copy()
                    environ["PYTHONDONTWRITEBYTECODE"] = "1"
                    
                    trim_proc = subprocess.Popen(
                        ffmpeg_cmd,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT,
                        text=True,
                        encoding='utf-8',
                        errors='replace',
                        env=environ,
                        startupinfo=startupinfo,
                        creationflags=creationflags
                    )
                    
                    for tline in trim_proc.stdout:
                        yield f"data: {json.dumps({'log': f'[ffmpeg] {tline.strip()}'})}\n\n"
                        
                    trim_proc.wait()
                    
                    if trim_proc.returncode == 0:
                        yield f"data: {json.dumps({'log': '> [FinFetcher] Trim successful! Replacing original file...'})}\n\n"
                        try:
                            if os.path.exists(final_file):
                                os.remove(final_file)
                            os.rename(trimmed_file, final_file)
                            yield f"data: {json.dumps({'log': '> [FinFetcher] Ready!'})}\n\n"
                        except Exception as e:
                            yield f"data: {json.dumps({'log': f'> [FinFetcher] Error replacing file: {e}'})}\n\n"
                    else:
                        yield f"data: {json.dumps({'log': f'> [FinFetcher] Trim failed with code {trim_proc.returncode}'})}\n\n"
                         
                except Exception as e:
                    yield f"data: {json.dumps({'log': f'> [FinFetcher] Trim error: {e}'})}\n\n"
            
            # Send final status
            if download_success:
                yield f"data: {json.dumps({'status': 'completed'})}\n\n"
            else:
                 # Error already sent above
                 pass
                 
        return Flask.response_class(generate(), mimetype='text/event-stream')

    except Exception as e:
        return jsonify({'error': str(e)}), 500


if __name__ == '__main__':
    # Get icon path (works for both dev and bundled exe)
    if getattr(sys, 'frozen', False):
        base_path = sys._MEIPASS
    else:
        base_path = os.path.dirname(os.path.abspath(__file__))
    
    # Try .ico first (better Windows compatibility), then .png
    icon_path = os.path.join(base_path, 'icon.ico')
    if not os.path.exists(icon_path):
        icon_path = os.path.join(base_path, 'icon.png')
    if not os.path.exists(icon_path):
        icon_path = None
    
    def set_window_icon():
        """Set window icon on Windows using ctypes (workaround for script mode)."""
        if os.name != 'nt' or icon_path is None:
            return
        try:
            import ctypes
            
            # Windows API constants
            WM_SETICON = 0x0080
            ICON_SMALL = 0
            ICON_BIG = 1
            IMAGE_ICON = 1
            LR_LOADFROMFILE = 0x0010
            LR_DEFAULTSIZE = 0x0040
            
            user32 = ctypes.windll.user32
            
            # Load icon from file
            hIcon = user32.LoadImageW(
                None, icon_path, IMAGE_ICON, 0, 0,
                LR_LOADFROMFILE | LR_DEFAULTSIZE
            )
            
            if hIcon:
                # Find window with retries
                hwnd = None
                for _ in range(10):
                    hwnd = user32.FindWindowW(None, "FinFetcher")
                    if hwnd:
                        break
                    time.sleep(0.1)
                
                if hwnd:
                    user32.SendMessageW(hwnd, WM_SETICON, ICON_SMALL, hIcon)
                    user32.SendMessageW(hwnd, WM_SETICON, ICON_BIG, hIcon)
        except Exception:
            pass  # Silently fail if icon can't be set
    
    api = Api()
    window = webview.create_window('FinFetcher', app, js_api=api, width=700, height=1200, resizable=True)
    
    # Set icon when window is shown
    def on_shown():
        time.sleep(0.3)
        set_window_icon()
    
    window.events.shown += on_shown
    webview.start()
