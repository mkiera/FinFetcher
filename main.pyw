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


# ============ Update Manager ============

class UpdateManager:
    """Manages checking for updates from GitHub releases and applying them."""

    GITHUB_API_URL = "https://api.github.com/repos/mkiera/FinFetcher/releases"
    CHECK_COOLDOWN_SECONDS = 3600  # 1 hour between automatic checks

    def __init__(self):
        self._config_file = os.path.join(FFmpegManager.get_app_data_dir(), 'config.json')
        self._config = self._load_config()

    def _load_config(self):
        """Load update-related config from the shared config file."""
        try:
            if os.path.exists(self._config_file):
                with open(self._config_file, 'r') as f:
                    return json.load(f)
        except Exception:
            pass
        return {}

    def _save_config(self):
        """Save update-related config back to the shared config file."""
        try:
            # Merge with existing config (don't overwrite ffmpeg_path etc.)
            existing = {}
            if os.path.exists(self._config_file):
                with open(self._config_file, 'r') as f:
                    existing = json.load(f)
            existing.update(self._config)
            with open(self._config_file, 'w') as f:
                json.dump(existing, f)
        except Exception:
            pass

    def get_current_version(self):
        """Read the current version from version.txt."""
        try:
            if getattr(sys, 'frozen', False):
                base_path = sys._MEIPASS
            else:
                base_path = os.path.dirname(os.path.abspath(__file__))
            version_file = os.path.join(base_path, 'version.txt')
            with open(version_file, 'r') as f:
                return f.read().strip()
        except Exception:
            return '0.0.0'

    @staticmethod
    def _parse_version(version_str):
        """Parse a version string into a comparable tuple.
        
        Supports formats:
          '1.2.3'           → stable
          '1.2.3b'          → pre-release (legacy bugfix beta)
          '1.2.3f-branch'   → pre-release (feature beta)
          '1.2.3b-branch'   → pre-release (bugfix beta)
          '1.2.3-beta'      → pre-release
          '1.2.3-rc1'       → pre-release
        
        Returns (major, minor, patch, is_stable) where is_stable is 1 for
        stable releases and 0 for pre-releases.
        """
        import re
        v = version_str.strip().lstrip('v')
        # Extract numeric major.minor.patch, treating any trailing
        # non-numeric suffix (f-branch, b-branch, -beta, etc.) as pre-release.
        m = re.match(r'^(\d+)(?:\.(\d+))?(?:\.(\d+))?([a-zA-Z\-].*)?$', v)
        if not m:
            return (0, 0, 0, 1)
        major = int(m.group(1)) if m.group(1) else 0
        minor = int(m.group(2)) if m.group(2) else 0
        patch = int(m.group(3)) if m.group(3) else 0
        is_stable = 0 if m.group(4) else 1
        return (major, minor, patch, is_stable)

    def _is_newer(self, remote_version, local_version):
        """Check if remote_version is newer than local_version."""
        remote = self._parse_version(remote_version)
        local = self._parse_version(local_version)
        # Compare (major, minor, patch) only — don't penalize pre-release
        return remote[:3] > local[:3]

    def _should_auto_check(self):
        """Check if enough time has passed since the last automatic check."""
        last_check = self._config.get('last_update_check')
        if not last_check:
            return True
        try:
            from datetime import datetime
            last_dt = datetime.fromisoformat(last_check)
            now = datetime.now()
            return (now - last_dt).total_seconds() >= self.CHECK_COOLDOWN_SECONDS
        except Exception:
            return True

    def _record_check(self):
        """Record that an update check just happened."""
        from datetime import datetime
        self._config['last_update_check'] = datetime.now().isoformat()
        self._save_config()

    def get_settings(self):
        """Return update-related settings."""
        return {
            'update_channel': self._config.get('update_channel', 'stable'),
            'auto_check_updates': self._config.get('auto_check_updates', True),
            'skipped_version': self._config.get('skipped_version', None),
        }

    def save_settings(self, settings):
        """Save update-related settings."""
        for key in ['update_channel', 'auto_check_updates', 'skipped_version']:
            if key in settings:
                self._config[key] = settings[key]
        self._save_config()

    def check_for_updates(self, force=False):
        """Check GitHub for a newer release.
        
        Args:
            force: If True, bypass the cooldown cache.
            
        Returns dict with update info or None if up-to-date/error.
        """
        # Respect cooldown unless forced
        if not force and not self._should_auto_check():
            return {'skipped': True, 'reason': 'cooldown'}

        include_prerelease = self._config.get('update_channel', 'stable') == 'prerelease'
        current_version = self.get_current_version()
        self._record_check()

        try:
            req = Request(self.GITHUB_API_URL, headers={
                'User-Agent': 'FinFetcher-Updater/1.0',
                'Accept': 'application/vnd.github.v3+json',
            })
            with urlopen(req, timeout=15) as response:
                releases = json.loads(response.read().decode('utf-8'))

            if not releases:
                return None

            # Find the best candidate release
            best = None
            for release in releases:
                tag = release.get('tag_name', '')
                is_prerelease = release.get('prerelease', False)
                is_draft = release.get('draft', False)

                if is_draft:
                    continue
                if is_prerelease and not include_prerelease:
                    continue

                version = tag.lstrip('v')
                if self._is_newer(version, current_version):
                    if best is None or self._is_newer(version, best['version']):
                        # Find the .exe asset
                        exe_asset = None
                        for asset in release.get('assets', []):
                            if asset['name'].endswith('.exe'):
                                exe_asset = asset
                                break

                        best = {
                            'version': version,
                            'tag': tag,
                            'prerelease': is_prerelease,
                            'html_url': release.get('html_url', ''),
                            'published_at': release.get('published_at', ''),
                            'exe_asset': {
                                'name': exe_asset['name'],
                                'url': exe_asset['browser_download_url'],
                                'size': exe_asset['size'],
                            } if exe_asset else None,
                        }

            if best:
                # Check if user skipped this version
                skipped = self._config.get('skipped_version')
                return {
                    'available': True,
                    'current_version': current_version,
                    'update': best,
                    'was_skipped': skipped == best['version'],
                }

            return {'available': False, 'current_version': current_version}

        except Exception as e:
            return {'error': str(e), 'current_version': current_version}

    def download_update(self, asset_url, asset_name, progress_callback=None):
        """Download an update asset to a temp directory.
        
        Returns the path to the downloaded file, or None on failure.
        """
        try:
            download_dir = os.path.join(FFmpegManager.get_app_data_dir(), 'updates')
            os.makedirs(download_dir, exist_ok=True)
            dest_path = os.path.join(download_dir, asset_name)

            req = Request(asset_url, headers={'User-Agent': 'FinFetcher-Updater/1.0'})

            with urlopen(req, timeout=120) as response:
                total_size = int(response.headers.get('Content-Length', 0))
                downloaded = 0
                chunk_size = 1024 * 64

                with open(dest_path, 'wb') as f:
                    while True:
                        chunk = response.read(chunk_size)
                        if not chunk:
                            break
                        f.write(chunk)
                        downloaded += len(chunk)

                        if progress_callback and total_size > 0:
                            percent = int((downloaded / total_size) * 100)
                            size_mb = downloaded / (1024 * 1024)
                            total_mb = total_size / (1024 * 1024)
                            progress_callback(percent, f"Downloading... {size_mb:.1f}/{total_mb:.1f} MB")

            return dest_path

        except Exception as e:
            if progress_callback:
                progress_callback(0, f"Download failed: {str(e)}")
            return None

    def apply_update(self, downloaded_exe_path):
        """Launch the updater helper and exit the app.
        
        The helper waits for this process to exit, swaps the exe, and relaunches.
        Returns True if the helper was launched successfully.
        """
        try:
            if getattr(sys, 'frozen', False):
                current_exe = sys.executable
                # Copy helper out of _MEIPASS to AppData so it doesn't block
                # PyInstaller's _MEI temp dir cleanup when the main app exits.
                bundled_helper = os.path.join(sys._MEIPASS, 'updater_helper.py')
                stable_helper = os.path.join(FFmpegManager.get_app_data_dir(), 'updater_helper.py')
                shutil.copy2(bundled_helper, stable_helper)
                helper_script = stable_helper
            else:
                current_exe = os.path.abspath(__file__)
                helper_script = os.path.join(os.path.dirname(current_exe), 'updater_helper.py')

            pid = os.getpid()

            # Hide console window on Windows
            startupinfo = None
            creationflags = 0
            if os.name == 'nt':
                startupinfo = subprocess.STARTUPINFO()
                startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
                creationflags = subprocess.CREATE_NO_WINDOW

            # Use python.exe for the helper (not pythonw.exe)
            python_exe = _get_python_exe() if not getattr(sys, 'frozen', False) else sys.executable

            subprocess.Popen(
                [python_exe, helper_script,
                 '--pid', str(pid),
                 '--old', current_exe,
                 '--new', downloaded_exe_path],
                startupinfo=startupinfo,
                creationflags=creationflags,
            )
            return True

        except Exception:
            return False


# Global UpdateManager instance
update_manager = UpdateManager()


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


# ============ Update API Endpoints ============

@app.route('/api/update/check', methods=['GET'])
def update_check():
    """Check for available updates from GitHub."""
    force = request.args.get('force', 'false').lower() == 'true'
    result = update_manager.check_for_updates(force=force)
    return jsonify(result or {'available': False})


@app.route('/api/update/download', methods=['GET'])
def update_download():
    """Download an update with SSE progress streaming."""
    asset_url = request.args.get('url')
    asset_name = request.args.get('name')

    if not asset_url or not asset_name:
        return jsonify({'error': 'Missing url or name parameters'}), 400

    def generate():
        try:
            yield f"data: {json.dumps({'percent': 0, 'status': 'Starting download...'})}\n\n"

            last_update = [None]

            def progress_cb(percent, status):
                last_update[0] = {'percent': percent, 'status': status}

            # Download in a thread so we can stream progress
            result = [None]
            def do_download():
                result[0] = update_manager.download_update(asset_url, asset_name, progress_cb)

            t = threading.Thread(target=do_download, daemon=True)
            t.start()

            while t.is_alive():
                if last_update[0]:
                    yield f"data: {json.dumps(last_update[0])}\n\n"
                    last_update[0] = None
                time.sleep(0.2)

            # Final update
            if last_update[0]:
                yield f"data: {json.dumps(last_update[0])}\n\n"

            if result[0]:
                yield f"data: {json.dumps({'percent': 100, 'status': 'Download complete!', 'success': True, 'path': result[0]})}\n\n"
            else:
                yield f"data: {json.dumps({'percent': 0, 'status': 'Download failed', 'success': False})}\n\n"

        except Exception as e:
            yield f"data: {json.dumps({'percent': 0, 'status': f'Error: {str(e)}', 'success': False})}\n\n"

    return Response(generate(), mimetype='text/event-stream')


@app.route('/api/update/apply', methods=['POST'])
def update_apply():
    """Apply a downloaded update (launches helper, then exits)."""
    data = request.json
    downloaded_path = data.get('path')

    if not downloaded_path or not os.path.exists(downloaded_path):
        return jsonify({'success': False, 'error': 'Downloaded file not found'})

    success = update_manager.apply_update(downloaded_path)

    if success:
        # Schedule app exit after responding
        def shutdown():
            time.sleep(1)
            os._exit(0)
        threading.Thread(target=shutdown, daemon=True).start()
        return jsonify({'success': True})
    else:
        return jsonify({'success': False, 'error': 'Failed to launch updater'})


@app.route('/api/update/settings', methods=['GET', 'POST'])
def update_settings():
    """Get or set update preferences."""
    if request.method == 'GET':
        settings = update_manager.get_settings()
        settings['current_version'] = update_manager.get_current_version()
        return jsonify(settings)
    else:
        data = request.json
        update_manager.save_settings(data)
        return jsonify({'success': True})


@app.route('/api/update/releases', methods=['GET'])
def update_releases():
    """List available releases from GitHub, filtered by channel."""
    channel = request.args.get('channel', 'stable')
    include_prerelease = channel == 'prerelease'
    current_version = update_manager.get_current_version()

    try:
        req = Request(update_manager.GITHUB_API_URL, headers={
            'User-Agent': 'FinFetcher-Updater/1.0',
            'Accept': 'application/vnd.github.v3+json',
        })
        with urlopen(req, timeout=15) as response:
            releases = json.loads(response.read().decode('utf-8'))

        result = []
        for release in releases:
            is_draft = release.get('draft', False)
            is_prerelease = release.get('prerelease', False)

            if is_draft:
                continue
            if is_prerelease and not include_prerelease:
                continue

            tag = release.get('tag_name', '')
            version = tag.lstrip('v')

            # Find exe asset
            exe_asset = None
            for asset in release.get('assets', []):
                if asset['name'].endswith('.exe'):
                    exe_asset = {
                        'name': asset['name'],
                        'url': asset['browser_download_url'],
                        'size': asset['size'],
                    }
                    break

            result.append({
                'version': version,
                'tag': tag,
                'prerelease': is_prerelease,
                'html_url': release.get('html_url', ''),
                'published_at': release.get('published_at', ''),
                'exe_asset': exe_asset,
                'is_current': version == current_version,
            })

        return jsonify({
            'releases': result,
            'current_version': current_version,
        })

    except Exception as e:
        return jsonify({'error': str(e), 'releases': [], 'current_version': current_version})


_cookie_opts_cache = None

def _get_python_exe():
    """Get the path to python.exe (not pythonw.exe) for subprocess calls."""
    exe = sys.executable
    if exe.lower().endswith('pythonw.exe'):
        # Replace pythonw.exe with python.exe
        candidate = exe[:-len('pythonw.exe')] + 'python.exe'
        if os.path.exists(candidate):
            return candidate
    return exe

def _extract_cookies_via_subprocess(browser):
    """Extract cookies from a Chromium browser via python.exe subprocess.
    
    On Windows, pythonw.exe can't access DPAPI for Chrome/Edge cookie decryption.
    Using python.exe in a subprocess works because it has proper console context.
    Saves cookies to a Netscape-format cookies.txt file.
    Returns the path to the cookies file, or None on failure.
    """
    cookies_dir = os.path.join(FFmpegManager.get_app_data_dir(), 'cookies')
    os.makedirs(cookies_dir, exist_ok=True)
    cookies_file = os.path.join(cookies_dir, f'{browser}_cookies.txt')
    
    # Python script to extract cookies and save to file
    script = f'''
import sys
try:
    import yt_dlp
    ydl_opts = {{
        'cookiesfrombrowser': ('{browser}',),
        'quiet': True,
        'no_warnings': True,
    }}
    with yt_dlp.YoutubeDL(ydl_opts) as ydl:
        ydl.cookiejar.save('{cookies_file.replace(chr(92), chr(92)*2)}', ignore_discard=True, ignore_expires=True)
    print('OK')
except Exception as e:
    print(f'FAIL:{{e}}', file=sys.stderr)
    sys.exit(1)
'''
    
    try:
        python_exe = _get_python_exe()
        
        # Hide console window on Windows
        startupinfo = None
        creationflags = 0
        if os.name == 'nt':
            startupinfo = subprocess.STARTUPINFO()
            startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
            creationflags = subprocess.CREATE_NO_WINDOW
        
        result = subprocess.run(
            [python_exe, '-c', script],
            capture_output=True, text=True, timeout=30,
            startupinfo=startupinfo, creationflags=creationflags,
        )
        
        if result.returncode == 0 and os.path.exists(cookies_file):
            return cookies_file
    except Exception:
        pass
    
    return None

def get_cookie_opts():
    """Get yt-dlp cookie options for YouTube authentication.
    
    Tries to extract cookies from an installed browser to bypass
    YouTube's bot detection (403 Forbidden errors).
    On Windows, Chromium cookies are extracted via a python.exe subprocess
    to work around DPAPI failures in pythonw.exe.
    Result is cached for the lifetime of the app.
    """
    global _cookie_opts_cache
    if _cookie_opts_cache is not None:
        return _cookie_opts_cache
    
    # On Windows, Chromium browsers need subprocess extraction due to DPAPI
    chromium_browsers = ['chrome', 'edge', 'brave', 'opera']
    non_chromium = ['firefox']
    
    if os.name == 'nt':
        # Try non-Chromium first (no DPAPI), then Chromium via subprocess
        for browser in non_chromium:
            try:
                test_opts = {
                    'cookiesfrombrowser': (browser,),
                    'quiet': True,
                    'no_warnings': True,
                    'extract_flat': True,
                }
                with yt_dlp.YoutubeDL(test_opts) as ydl:
                    ydl.extract_info('https://www.youtube.com/watch?v=jNQXAC9IVRw', download=False)
                    _cookie_opts_cache = {'cookiesfrombrowser': (browser,)}
                    return _cookie_opts_cache
            except Exception:
                continue
        
        # Try Chromium browsers via subprocess
        for browser in chromium_browsers:
            cookies_file = _extract_cookies_via_subprocess(browser)
            if cookies_file:
                _cookie_opts_cache = {'cookiefile': cookies_file}
                return _cookie_opts_cache
    else:
        # Non-Windows: try all browsers directly
        for browser in chromium_browsers + non_chromium:
            try:
                test_opts = {
                    'cookiesfrombrowser': (browser,),
                    'quiet': True,
                    'no_warnings': True,
                    'extract_flat': True,
                }
                with yt_dlp.YoutubeDL(test_opts) as ydl:
                    ydl.extract_info('https://www.youtube.com/watch?v=jNQXAC9IVRw', download=False)
                    _cookie_opts_cache = {'cookiesfrombrowser': (browser,)}
                    return _cookie_opts_cache
            except Exception:
                continue
    
    # No browser cookies available — proceed without them
    _cookie_opts_cache = {}
    return _cookie_opts_cache


def get_video_info(url, flat=True):
    """Fetch video metadata using yt-dlp Python API."""
    ydl_opts = {
        'quiet': True,
        'no_warnings': True,
        'extract_flat': flat,
    }
    ydl_opts.update(get_cookie_opts())
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
        ydl_opts.update(get_cookie_opts())
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


@app.route('/api/stream', methods=['POST'])
def get_stream_url():
    """Get direct stream URL for video playback without downloading."""
    data = request.json
    url = data.get('url')
    
    if not url:
        return jsonify({'error': 'No URL provided'}), 400
    
    try:
        # Configure yt-dlp to get streamable URL
        # Prefer formats with both video+audio in single stream for HTML5 compatibility
        ydl_opts = {
            'quiet': True,
            'no_warnings': True,
            'format': 'best[ext=mp4]/best',  # Prefer mp4 for browser compatibility
        }
        ydl_opts.update(get_cookie_opts())
        
        with yt_dlp.YoutubeDL(ydl_opts) as ydl:
            info = ydl.extract_info(url, download=False)
            
            # Get the direct URL
            stream_url = info.get('url')
            
            # If no direct URL, try to get from requested_formats
            if not stream_url and info.get('requested_formats'):
                # For merged formats, we need to find a single stream format
                # Fall back to finding a format with both video and audio
                formats = info.get('formats', [])
                
                # Find best format with both video and audio
                best_combined = None
                for f in formats:
                    has_video = f.get('vcodec') and f.get('vcodec') != 'none'
                    has_audio = f.get('acodec') and f.get('acodec') != 'none'
                    is_mp4 = f.get('ext') == 'mp4'
                    
                    if has_video and has_audio:
                        if not best_combined:
                            best_combined = f
                        elif is_mp4 and best_combined.get('ext') != 'mp4':
                            best_combined = f
                        elif (f.get('height', 0) or 0) > (best_combined.get('height', 0) or 0):
                            if is_mp4 or best_combined.get('ext') != 'mp4':
                                best_combined = f
                
                if best_combined:
                    stream_url = best_combined.get('url')
            
            if not stream_url:
                return jsonify({'error': 'Could not find streamable URL'}), 400
            
            return jsonify({
                'stream_url': stream_url,
                'title': info.get('title', 'Unknown'),
                'duration': info.get('duration', 0),
                'thumbnail': info.get('thumbnail', '')
            })
            
    except Exception as e:
        return jsonify({'error': str(e)}), 500


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
    ydl_opts.update(get_cookie_opts())
    
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
                    
                    # Detect if this is an audio-only file (MP3) or video
                    is_audio_file = ext.lower() in ['.mp3', '.m4a', '.aac', '.flac', '.wav', '.ogg', '.opus']
                    
                    if is_audio_file:
                        # Audio-only trimming - use appropriate audio codec
                        if ext.lower() == '.mp3':
                            audio_codec = ['-c:a', 'libmp3lame', '-b:a', '192k']
                        elif ext.lower() in ['.m4a', '.aac']:
                            audio_codec = ['-c:a', 'aac', '-b:a', '192k']
                        elif ext.lower() == '.flac':
                            audio_codec = ['-c:a', 'flac']
                        elif ext.lower() == '.opus':
                            audio_codec = ['-c:a', 'libopus', '-b:a', '128k']
                        elif ext.lower() == '.ogg':
                            audio_codec = ['-c:a', 'libvorbis', '-q:a', '5']
                        else:
                            audio_codec = ['-c:a', 'copy']  # WAV or unknown - just copy
                        
                        ffmpeg_cmd = [
                            ffmpeg_exe, '-y',
                            '-i', final_file,
                            '-ss', trim_start,
                            '-to', trim_end,
                        ] + audio_codec + [trimmed_file]
                    else:
                        # Video trimming - use video and audio codecs
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
    # Set AppUserModelID for proper taskbar icon (must be done before window creation)
    # This prevents Windows from showing Python's icon in the taskbar
    if os.name == 'nt':
        try:
            import ctypes
            ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID('FinFetcher.App.1')
        except Exception:
            pass
    
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
