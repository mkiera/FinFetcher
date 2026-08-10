"""
FinFetcher 🦭
A friendly video & music downloader desktop application.
Built with Flask + PyWebView for native desktop experience.
"""


import os
import sys
import json
import ssl
import subprocess
import webview
import threading
import queue
import time
import zipfile
import tempfile
import shutil
import yt_dlp
from yt_dlp.postprocessor import PostProcessor
from yt_dlp.utils import DownloadCancelled, download_range_func
from flask import Flask, request, jsonify, send_from_directory, Response
from urllib.request import urlopen, Request
from urllib.error import URLError
from urllib.parse import urlparse

# certifi ships with yt-dlp; the explicit import also tells PyInstaller to
# bundle the CA bundle into the exe.
try:
    import certifi
except ImportError:
    certifi = None

_ssl_context = None
_ssl_ca_source = None


def get_ssl_context():
    """SSL context used for every HTTPS request this app makes.

    Windows' own certificate store can hold an expired root that OpenSSL
    then picks when building a chain, so verification fails for hosts the
    rest of the system trusts fine (nightly.link is one). certifi's bundle
    avoids that; fall back to the system store if it isn't available.

    The fallback is a degraded state, not a normal one — get_ca_source()
    reports which store is in use so the debug panel and any TLS failure
    can say so instead of failing mysteriously.
    """
    global _ssl_context, _ssl_ca_source
    if _ssl_context is None:
        try:
            ca_file = certifi.where()
            if not os.path.exists(ca_file):
                raise FileNotFoundError(ca_file)
            _ssl_context = ssl.create_default_context(cafile=ca_file)
            _ssl_ca_source = f'certifi ({ca_file})'
        except Exception as e:
            _ssl_context = ssl.create_default_context()
            _ssl_ca_source = f'system certificate store — certifi unavailable ({e})'
    return _ssl_context


def get_ca_source():
    """Describe which CA store HTTPS verification is using."""
    get_ssl_context()
    return _ssl_ca_source


def using_system_ca():
    """True when we fell back to the OS store, which may hold expired roots."""
    return get_ca_source().startswith('system')


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
        # Only honour the custom path while it still holds an ffmpeg binary —
        # a stale folder would otherwise shadow a fresh AppData install.
        ffmpeg_name = 'ffmpeg.exe' if os.name == 'nt' else 'ffmpeg'
        if self._custom_path and os.path.exists(os.path.join(self._custom_path, ffmpeg_name)):
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
        """Save configuration to disk.

        Merges into the existing file — update settings live in the same
        config.json and must not be wiped by an ffmpeg path change.
        """
        try:
            existing = {}
            if os.path.exists(self._config_file):
                try:
                    with open(self._config_file, 'r') as f:
                        existing = json.load(f)
                except Exception:
                    existing = {}
            existing['ffmpeg_path'] = self._custom_path
            with open(self._config_file, 'w') as f:
                json.dump(existing, f)
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
                with urlopen(req, timeout=60, context=get_ssl_context()) as response:
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


def ensure_ffmpeg_discoverable():
    """Make our ffmpeg visible to the yt-dlp probes that ignore ffmpeg_location.

    yt-dlp only downloads a byte range instead of the whole video when it picks
    FFmpegFD, and it only picks FFmpegFD when FFmpegFD.available() is true
    (yt_dlp/downloader/__init__.py:92). That call is
    `FFmpegPostProcessor().available` with no downloader attached
    (yt_dlp/downloader/external.py:458-462), and with no downloader
    PostProcessor.get_param returns its default, so _determine_executables
    never sees the ffmpeg_location we put in the options dict and falls back to
    the bare names 'ffmpeg'/'ffprobe', which are resolved through PATH
    (yt_dlp/postprocessor/ffmpeg.py:102-107). This app keeps ffmpeg in
    %APPDATA%\\FinFetcher\\ffmpeg, which is on nobody's PATH, so a ranged
    download was refused outright — "You have requested downloading the video
    partially, but ffmpeg is not installed" (YoutubeDL.py:3440-3445) — and this
    app's fallback then fetched the whole video and re-encoded it, which is the
    exact cost fast trim exists to avoid.

    Both mechanisms are used because neither covers the other:
      - PATH is process-wide and permanent, so it fixes the probe on every
        thread, including any that ran before this call.
      - FFmpegPostProcessor._ffmpeg_location is the ContextVar yt-dlp's own CLI
        sets for precisely this reason (yt_dlp/__init__.py:974). It pins the
        exact binary rather than whatever PATH happens to resolve, and it is
        the only route left once a probe has already cached a miss:
        _version_cache is keyed by path (ffmpeg.py:130-135), so a bare 'ffmpeg'
        that failed once stays failed for the life of the process no matter
        what PATH says afterwards. Being a ContextVar it is per-thread, so this
        has to be called again on the thread that runs the download.

    Does nothing when ffmpeg is not installed: handing yt-dlp a directory that
    does not exist makes _determine_executables return {} and stop consulting
    PATH at all, which would break the one case that already works — a user
    whose ffmpeg came from somewhere else entirely.

    Returns True when ffmpeg was found and pointed at.
    """
    if not ffmpeg_manager.is_installed():
        return False

    ffmpeg_dir = ffmpeg_manager.get_ffmpeg_dir()

    entries = [p for p in os.environ.get('PATH', '').split(os.pathsep) if p]
    if not any(os.path.normcase(p) == os.path.normcase(ffmpeg_dir) for p in entries):
        os.environ['PATH'] = os.pathsep.join([ffmpeg_dir, *entries])

    try:
        from yt_dlp.postprocessor.ffmpeg import FFmpegPostProcessor
        FFmpegPostProcessor._ffmpeg_location.set(ffmpeg_dir)
    except Exception:
        # A yt-dlp without that ContextVar. PATH above still answers the
        # probe, so this is not worth failing a download over.
        pass
    return True


# Do it once at startup as well, before anything can probe for ffmpeg and cache
# the miss that no later PATH change could undo.
ensure_ffmpeg_discoverable()


# ============ Update Manager ============

class UpdateManager:
    """Manages checking for updates from GitHub releases and applying them."""

    GITHUB_API_URL = "https://api.github.com/repos/mkiera/FinFetcher/releases"
    CHECK_COOLDOWN_SECONDS = 3600  # 1 hour between automatic checks
    MIN_UPDATE_VERSION = (1, 2, 0)  # Don't offer versions older than this (no updater)

    def __init__(self):
        self._config_file = os.path.join(FFmpegManager.get_app_data_dir(), 'config.json')
        self._config = self._load_config()
        self.last_download_error = None
        self._remove_legacy_updater()
        self._clear_updates_dir()

    # Files older builds wrote into the app data directory to update
    # themselves. Both were this app's own doing, and nothing generates or runs
    # either any more.
    LEGACY_UPDATER_FILES = (
        # The exe-swapping batch script builds up to 1.2.4 used. It deleted
        # itself on its last line, so one only survives an update that was
        # interrupted part way through.
        'update.bat',
        # The Python helper that preceded that batch script, copied out of the
        # PyInstaller temp dir so it would outlive the app it was replacing.
        'updater_helper.py',
    )

    @staticmethod
    def _remove_legacy_updater():
        """Clear away the updater scripts builds up to 1.2.4 left in AppData.

        A stale copy sitting there is one more thing for a future reader to
        wonder about. Best effort: if something still has one open the delete
        fails, and it will go on the next start.
        """
        for name in UpdateManager.LEGACY_UPDATER_FILES:
            try:
                legacy = os.path.join(FFmpegManager.get_app_data_dir(), name)
                if os.path.isfile(legacy):
                    os.remove(legacy)
            except Exception:
                pass

    @staticmethod
    def _clear_updates_dir():
        """Delete the installers left behind by earlier update attempts.

        Everything in the updates directory was put there by download_update,
        so this only ever removes this app's own downloads — nothing else
        writes to it. They cannot be cleaned up when they are used: apply_update
        hands the file to Windows and the app exits immediately so the installer
        can replace it, which leaves ~100 MB per update sitting in AppData
        forever. Startup is the next moment at which they are provably
        finished with, and that is where the batch updater these replaced did
        its own tidying up.

        Best effort. The obvious failure is the installer that just ran us
        still holding its own exe open, and one more launch will clear it.
        """
        updates_dir = UpdateManager.get_updates_dir()
        try:
            entries = os.listdir(updates_dir)
        except OSError:
            return  # never downloaded anything, or cannot be read
        for entry in entries:
            target = os.path.join(updates_dir, entry)
            try:
                # A zipped CI artifact extracts into a subdirectory of its own
                if os.path.isdir(target) and not os.path.islink(target):
                    shutil.rmtree(target, ignore_errors=True)
                else:
                    os.remove(target)
            except OSError:
                pass

    def _load_config(self):
        """Load update-related config from the shared config file."""
        try:
            if os.path.exists(self._config_file):
                with open(self._config_file, 'r') as f:
                    return json.load(f)
        except Exception:
            pass
        return {}

    # Config keys this manager owns — everything else in config.json belongs
    # to another component and must be left exactly as found on disk.
    CONFIG_KEYS = ('update_channel', 'auto_check_updates',
                   'skipped_version', 'last_update_check')

    def _save_config(self):
        """Save update-related config back to the shared config file."""
        try:
            # Merge with existing config (don't overwrite ffmpeg_path etc.)
            existing = {}
            if os.path.exists(self._config_file):
                try:
                    with open(self._config_file, 'r') as f:
                        existing = json.load(f)
                except Exception:
                    existing = {}
            # Only write back our own keys — self._config is a startup snapshot
            # of the whole file and would otherwise restore stale values.
            for key in self.CONFIG_KEYS:
                if key in self._config:
                    existing[key] = self._config[key]
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
        # Compare (major, minor, patch, is_stable) so the stable release wins
        # over a pre-release of the same version — 1.2.2 updates 1.2.2b-foo.
        return remote[:4] > local[:4]

    @staticmethod
    def _is_installer_name(name):
        """True when an asset is the Inno Setup installer, not a bare app exe.

        From 1.2.5 the app ships as a directory of files installed by
        FinFetcher-Setup.exe, so the installer is the only thing that knows
        where the app lives and how to replace all of it. Releases up to 1.2.4
        shipped a single self-contained FinFetcher.exe, which running would
        merely start an unpacked copy of that old version out of the updates
        folder. The name is the signal because it is the one we control: the
        release asset and the CI artifact are both FinFetcher-Setup.exe.
        """
        base = os.path.basename(name or '').lower()
        return base.endswith('.exe') and 'setup' in base

    @staticmethod
    def _pick_exe_asset(assets):
        """Choose which .exe asset of a release to offer.

        Prefer the installer when a release carries more than one .exe, but
        still fall back to the first one so an older, bare-exe release stays
        visible in the list. apply_update is what refuses to run it, with an
        explanation the user can act on — silently hiding those releases would
        take away the only route back off a bad version.
        """
        exe_assets = [a for a in assets
                      if str(a.get('name', '')).lower().endswith('.exe')]
        if not exe_assets:
            return None
        for asset in exe_assets:
            if UpdateManager._is_installer_name(asset['name']):
                return asset
        return exe_assets[0]

    @staticmethod
    def get_updates_dir():
        """The one directory an update may be downloaded to and run from."""
        return os.path.join(FFmpegManager.get_app_data_dir(), 'updates')

    @staticmethod
    def is_inside_updates_dir(path):
        """True when path really resolves to somewhere inside the updates dir.

        Guards the only place this app runs a file it fetched off the network,
        so it resolves symlinks and junctions before comparing rather than
        trusting the string it was handed.
        """
        updates_dir = os.path.realpath(UpdateManager.get_updates_dir())
        try:
            return os.path.commonpath(
                [os.path.realpath(path), updates_dir]) == updates_dir
        except (ValueError, OSError, TypeError):  # different drives, or not a path
            return False

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
        # Respect the user's preference and the cooldown unless forced
        if not force and not self._config.get('auto_check_updates', True):
            return {'skipped': True, 'reason': 'disabled'}
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
            with urlopen(req, timeout=15, context=get_ssl_context()) as response:
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

                # Skip versions older than the minimum (no updater support)
                parsed = self._parse_version(tag)
                if parsed[:3] < self.MIN_UPDATE_VERSION:
                    continue

                version = tag.lstrip('v')
                if self._is_newer(version, current_version):
                    if best is None or self._is_newer(version, best['version']):
                        exe_asset = self._pick_exe_asset(release.get('assets', []))

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
                                # False means self-update will refuse it — see
                                # _is_installer_name.
                                'is_installer': self._is_installer_name(exe_asset['name']),
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

        If the downloaded file is a zip, extracts the installer from it.
        Returns the path to the downloaded/extracted exe, or None on failure.
        On failure the reason is left in self.last_download_error so the
        caller can show it instead of a bare "Download failed".
        """
        self.last_download_error = None
        try:
            download_dir = self.get_updates_dir()
            os.makedirs(download_dir, exist_ok=True)
            # Never let the asset name steer the write outside the updates dir
            dest_path = os.path.join(download_dir, os.path.basename(asset_name))

            req = Request(asset_url, headers={'User-Agent': 'FinFetcher-Updater/1.0'})

            with urlopen(req, timeout=120, context=get_ssl_context()) as response:
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

            # A read loop that stops on the first empty chunk cannot tell the
            # end of the body from a connection cut, and the file this produces
            # is one we go on to *execute* as an installer. Compare it with the
            # length the server promised and refuse anything short, rather than
            # reporting a truncated installer as a finished download.
            #
            # Only when there is something to compare against: a chunked
            # response carries no Content-Length, and total_size is 0 then. That
            # is not a failure, just an unverifiable transfer — the guards in
            # apply_update still stand behind it.
            if total_size > 0 and downloaded != total_size:
                _remove_file_with_retry(dest_path)
                self.last_download_error = (
                    f'The download stopped early — got {downloaded} bytes of the '
                    f'{total_size} the server said it would send. The file was '
                    'incomplete and has been deleted; please try again.')
                if progress_callback:
                    progress_callback(0, self.last_download_error)
                return None

            # If it's a zip (e.g. GitHub Actions artifact), extract the exe
            if asset_name.lower().endswith('.zip'):
                if progress_callback:
                    progress_callback(95, 'Extracting update...')
                with zipfile.ZipFile(dest_path, 'r') as zf:
                    exe_files = [n for n in zf.namelist() if n.lower().endswith('.exe')]
                    if not exe_files:
                        self.last_download_error = 'No exe found in the downloaded zip'
                        if progress_callback:
                            progress_callback(0, self.last_download_error)
                        return None
                    # A onedir build zips up as a whole tree of exes (the app,
                    # plus whatever ships beside it), so "the first one" is no
                    # longer good enough — take the installer when it is there.
                    wanted = next((n for n in exe_files
                                   if self._is_installer_name(n)), exe_files[0])
                    zf.extract(wanted, download_dir)
                    extracted_path = os.path.join(download_dir, wanted)
                # Clean up the zip
                os.remove(dest_path)
                return extracted_path

            return dest_path

        except Exception as e:
            self.last_download_error = str(e)
            # A verification failure while on the OS store is the known
            # bad state — say so rather than leaving a raw SSL error.
            if 'CERTIFICATE_VERIFY_FAILED' in str(e) and using_system_ca():
                self.last_download_error += (
                    ' — this build has no bundled CA store, so it is using the '
                    'Windows certificate store, which has an expired root. '
                    'Please download the update manually.'
                )
            if progress_callback:
                progress_callback(0, f"Download failed: {self.last_download_error}")
            return None

    # Switches handed to the Inno Setup installer for an unattended update.
    # Semantics are from the Inno Setup help, "Setup Command Line Parameters":
    #   /SILENT   hides the wizard but still shows the progress window, so the
    #             user sees the update happening after our window disappears.
    #             /VERYSILENT would leave a blank screen that reads as a crash.
    #   /SP-      skips the "This will install..." prompt; message boxes are
    #             NOT suppressed (no /SUPPRESSMSGBOXES) because once we have
    #             exited, a message box is the only way the installer can tell
    #             the user something went wrong.
    #   /NOCANCEL cancelling half way through would leave a half-written app
    #             directory and no running app to explain it.
    #   /NORESTART never reboot the machine on our behalf.
    #   /CLOSEAPPLICATIONS  we exit before the installer reaches the file copy,
    #             but a second FinFetcher window, or simply losing the race,
    #             would otherwise leave files locked. This is Setup's default
    #             anyway; passing it means the update still works if the
    #             installer script ever sets CloseApplications=no.
    #   /NORESTARTAPPLICATIONS  Setup only restarts applications that called
    #             RegisterApplicationRestart, which this app does not, so
    #             /RESTARTAPPLICATIONS could never relaunch us — see the
    #             RestartApplications directive in the Inno Setup help. Saying
    #             "no" explicitly keeps the relaunch owned by exactly one
    #             thing (the installer's own post-install launch) so a future
    #             restart registration cannot start a second window.
    # Deliberately NOT passed: /DIR and /TASKS. Setup's UsePreviousAppDir and
    # UsePreviousTasks both default to yes, so leaving them off keeps the
    # user's install location and their desktop-shortcut choice; /TASKS would
    # reset the task selection to only what we listed.
    INSTALLER_SWITCHES = (
        '/SILENT',
        '/SP-',
        '/NOCANCEL',
        '/NORESTART',
        '/CLOSEAPPLICATIONS',
        '/NORESTARTAPPLICATIONS',
    )

    # How long to watch the installer before believing it started. Setup
    # refuses (bad file, blocked by policy, "cannot proceed") by exiting almost
    # immediately, so a short watch catches that while it is still cheap.
    #
    # It cannot usefully be much longer, and lengthening it would not buy the
    # certainty it looks like it would: Setup's "Preparing to Install" stage is
    # where /CLOSEAPPLICATIONS shuts this app down, so a watch that ran on into
    # it would be waiting for an answer while being closed for the privilege.
    INSTALLER_START_TIMEOUT = 1.5

    # What Setup's exit codes mean, from the Inno Setup help, "Setup Exit
    # Codes". 0 is absent because it is success and is handled on its own.
    INSTALLER_EXIT_CODES = {
        1: 'It failed to initialise.',
        2: 'It was cancelled before the installation started.',
        3: 'A fatal error occurred while it was preparing to install.',
        4: 'A fatal error occurred during the installation.',
        5: 'It was cancelled during the installation.',
        6: 'It was terminated by a debugger.',
        7: 'It decided it could not proceed with the installation.',
        8: 'It decided it could not proceed until the system is restarted.',
    }

    # Setup writes this immediately before putting a dialog on screen...
    SETUP_LOG_MSGBOX_PREFIX = 'Message box ('
    # ...and this once somebody has answered one.
    SETUP_LOG_ANSWERED_PREFIX = 'User chose '

    @staticmethod
    def get_update_log_path():
        """Where the installer is told to write its log.

        Worth naming: it is the only account of an update that survives this
        app exiting, so every message about a failed update points at it.
        """
        return os.path.join(FFmpegManager.get_app_data_dir(), 'update.log')

    @staticmethod
    def _read_setup_log(log_path):
        """Setup's /LOG as a list of messages, continuations folded in.

        Each message is written as "<timestamp>   <text>"; one that spans lines
        has its continuations indented and carrying no timestamp of their own.
        The file is UTF-8 with a BOM. An empty list means Setup has not opened
        it yet, which is not the same as nothing being wrong.
        """
        try:
            with open(log_path, encoding='utf-8-sig', errors='replace') as f:
                lines = f.read().splitlines()
        except OSError:
            return []  # not created yet, or not readable

        messages = []
        for line in lines:
            text = line.strip()
            if not text:
                continue
            if messages and not line[:1].isdigit():
                messages[-1] += ' ' + text  # a continuation of the one above
            else:
                # Drop the timestamp Setup stamps on the front of every message
                _, _, remainder = text.partition('   ')
                messages.append(remainder.strip() or text)
        return messages

    @classmethod
    def _pending_dialog(cls, messages):
        """The message box Setup is sitting on right now, or None.

        Setup logs "Message box (<buttons>): <text>" and flushes it *before* the
        box appears, and logs "User chose ..." only once the box has been
        answered — so an unanswered one at the end of the log is a Setup that
        has stopped and is waiting for a person. Checked against the Inno Setup
        6.7.3 this project builds with, by running a Setup that opens a dialog
        with the switches below and reading its log while the box was still up:
        the entry was there 11 ms in and the file did not change for as long as
        the box stayed on screen.

        This is the one honest signal available in the moment before we exit.
        /SUPPRESSMSGBOXES is deliberately not passed (see INSTALLER_SWITCHES)
        and a silent update has nothing it needs to ask, so any dialog at all
        means something has gone wrong.
        """
        pending = None
        for message in messages:
            if message.startswith(cls.SETUP_LOG_MSGBOX_PREFIX):
                pending = message
            elif message.startswith(cls.SETUP_LOG_ANSWERED_PREFIX):
                pending = None
        if not pending:
            return None
        # Drop the "Message box (OK):" preamble — only the words matter here
        _, _, text = pending.partition(':')
        return text.strip() or pending

    def apply_update(self, installer_path):
        """Run a downloaded installer so it can replace this installation.

        The app ships as a directory of files, so there is nothing to swap in
        place any more: replacing FinFetcher.exe alone would leave a new
        bootloader next to a stale _internal folder and the app would not
        start. The installer is what knows the whole layout, so all this does
        is start it and get out of the way; the caller exits once this returns
        success, because the installer cannot overwrite files we still hold.

        Only supported for the packaged app. Running from source there is
        nothing to install over — the "old exe" would be main.pyw itself.

        Returns (started, message). started is False when nothing was launched
        or when the installer has visibly stopped on a dialog, and the app must
        then stay up: a user with neither the old app nor the new one has no way
        back, so every refusal here is one the UI can show and act on.

        started True does NOT mean the update worked, and the message says so.
        Only one outcome here is ever certain — Setup exiting 0 inside the watch
        below — because the installer cannot replace these files until this
        process is gone, and once it is gone there is nothing left to notice a
        failure, let alone report one. What is left is to spend the moment
        before exiting on the two things Setup will actually tell us: the exit
        code if it gives up straight away, and its /LOG if it stops to ask
        something. Beyond that the ambiguity is the design's, not a gap in the
        checking, and the wording is honest about it rather than promising.
        """
        if not getattr(sys, 'frozen', False):
            return False, ('Self-update is only available in the packaged app — '
                           'please download the new version manually.')

        # Only ever run something we downloaded ourselves, into our own folder
        if not self.is_inside_updates_dir(installer_path):
            return False, 'Update file is outside the updates folder'

        if not os.path.isfile(installer_path):
            return False, 'Downloaded file not found'

        if not self._is_installer_name(installer_path):
            # A bare exe from v1.2.4 or earlier: running it would start that
            # old version out of the updates folder and change nothing about
            # the installed copy, which looks exactly like a successful update
            # until the next restart. Refuse and say what to do instead.
            return False, (
                f'{os.path.basename(installer_path)} is not the FinFetcher installer. '
                'Builds up to 1.2.4 shipped as a single stand-alone exe, which cannot '
                'replace an installed copy — to go back to one, uninstall FinFetcher '
                'from Windows Settings first, then run that download yourself. It is '
                f'kept at {installer_path}.')

        log_path = self.get_update_log_path()

        # Start from no log at all, so whatever is in it afterwards provably
        # belongs to this attempt. Setup does overwrite the file when it opens
        # it — checked by pointing two runs of the same installer at one log,
        # which left one "Log opened." in it — but it only opens it once it has
        # got that far, and how far it got is the very thing being read.
        # One attempt, no retries: a locked update.log is not worth stalling an
        # update for, and Setup would overwrite it anyway.
        _remove_file_with_retry(log_path, attempts=1)

        creationflags = 0
        if os.name == 'nt':
            # Outlive our own exit, and take no console window with it
            creationflags = subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP

        try:
            # cwd stays in AppData: a working directory inside the install
            # folder would lock that folder against the very files the
            # installer has to replace.
            proc = subprocess.Popen(
                [installer_path, *self.INSTALLER_SWITCHES, f'/LOG={log_path}'],
                cwd=FFmpegManager.get_app_data_dir(),
                creationflags=creationflags,
                close_fds=True,
            )
        except Exception as e:
            # Missing file, blocked by AV or policy, not an executable at all —
            # whatever it was, this app is still running and has to say so.
            return False, f'Could not start the installer: {e}'

        try:
            code = proc.wait(timeout=self.INSTALLER_START_TIMEOUT)
        except subprocess.TimeoutExpired:
            code = None  # still running, which is the normal case

        messages = self._read_setup_log(log_path)
        asking = self._pending_dialog(messages)

        if code is not None and code != 0:
            return False, ' '.join(part for part in (
                f'The installer stopped straight away (exit code {code}).',
                self.INSTALLER_EXIT_CODES.get(code, ''),
                f'It said: "{asking}"' if asking else '',
                f'Its log is at {log_path}.') if part)

        if code == 0:
            # The one outcome that is not a guess. 0 is "Setup was successfully
            # run to completion", and the installer's [Run] entry has already
            # started the new version, so this process is now simply the stale
            # one and nothing here holds a file the installer still wants.
            return True, ('The update is installed, and the new version has already '
                          'started. FinFetcher will close now.')

        if asking:
            # Setup is on screen waiting for an answer it will wait for forever.
            # Exiting into that would leave the user with a dialog and no app to
            # explain it — which is exactly the case that used to look identical
            # to a healthy install — so stay up and say what it is asking.
            return False, (
                'The installer has stopped to ask something and is waiting for an '
                f'answer: "{asking}" FinFetcher has stayed open rather than leave '
                'you with a dialog and nothing else. Deal with the installer '
                f'window first; its log is at {log_path}.')

        # Running, and it has not said anything is wrong. That is the whole of
        # what can be known from here, so say that and no more.
        return True, (
            'The installer is running'
            + ('' if messages else ' but has not written to its log yet')
            + ', and FinFetcher has to close so it can replace these files. This '
            'app cannot see how the install ends — if FinFetcher does not reopen '
            f'on the new version, the installer wrote what happened to {log_path}.')


# Global UpdateManager instance
update_manager = UpdateManager()


# ============ App Settings ============

class AppSettings:
    """Stores the download preferences the Settings panel exposes.

    Shares config.json with FFmpegManager and UpdateManager, so it follows the
    same rule as UpdateManager._save_config: read the file, write back only
    the keys this class owns, leave everything else exactly as found.
    """

    # The whole settings contract, key -> default. Iteration order is the
    # order the API hands back, and CONFIG_KEYS is derived from it so the
    # write whitelist can never drift away from the schema.
    DEFAULTS = {
        'concurrent_fragments': 4,
        'rate_limit_kbps': 0,
        # Off by default: yt-dlp's archive key is extractor+id only, so an
        # entry fetched as audio counts as "done" for a later video run.
        # Opting in has to be a deliberate choice.
        'use_download_archive': False,
        # On by default so trimming stays frame-accurate, as it was before
        # range downloads. Turning it off snaps cuts to the nearest keyframe
        # (faster, no re-encode at the boundaries).
        'precise_trim': True,
        # Sits in the same settings panel as everything else here, so it has to
        # survive a restart like everything else here. The download request still
        # carries its own log_to_file flag; this is the remembered default.
        'log_to_file': False,
        'container': 'mp4',
        'audio_format': 'mp3',
        'audio_quality': '0',
        'subtitles_enabled': False,
        'subtitle_langs': 'en',
        'subtitles_auto': False,
        'embed_subtitles': True,
        'sponsorblock_enabled': False,
        'sponsorblock_categories': ['sponsor', 'selfpromo', 'interaction'],
        'embed_thumbnail': True,
        'embed_metadata': True,
        'embed_chapters': True,
        'split_chapters': False,
    }

    CONFIG_KEYS = tuple(DEFAULTS)

    CONTAINERS = ('mp4', 'mkv', 'webm')
    AUDIO_FORMATS = ('mp3', 'm4a', 'opus', 'flac', 'wav')
    # Every category yt-dlp's SponsorBlock post-processor understands
    SPONSORBLOCK_CATEGORIES = (
        'sponsor', 'intro', 'outro', 'selfpromo', 'preview',
        'filler', 'interaction', 'music_offtopic', 'poi_highlight',
    )
    MAX_CONCURRENT_FRAGMENTS = 16
    # 1 GB/s. Not a real constraint, just a ceiling so a mistyped value
    # can't turn into a nonsense byte rate.
    MAX_RATE_LIMIT_KBPS = 1024 * 1024

    def __init__(self):
        self._config_file = os.path.join(FFmpegManager.get_app_data_dir(), 'config.json')
        self._config = self._load_config()

    def _load_config(self):
        """Load the shared config file."""
        try:
            if os.path.exists(self._config_file):
                with open(self._config_file, 'r') as f:
                    config = json.load(f)
                    if isinstance(config, dict):
                        return config
        except Exception:
            pass
        return {}

    def _save_config(self):
        """Save these settings back to the shared config file."""
        try:
            existing = {}
            if os.path.exists(self._config_file):
                try:
                    with open(self._config_file, 'r') as f:
                        existing = json.load(f)
                    if not isinstance(existing, dict):
                        existing = {}
                except Exception:
                    existing = {}
            # Only write back our own keys — ffmpeg_path and the update keys
            # live in this file too and self._config is a startup snapshot of
            # the whole thing, which would restore stale values for them.
            for key in self.CONFIG_KEYS:
                if key in self._config:
                    existing[key] = self._config[key]
            with open(self._config_file, 'w') as f:
                json.dump(existing, f)
        except Exception:
            pass

    @staticmethod
    def _as_bool(value, fallback):
        """Coerce a checkbox value; JSON may carry it as a string or number."""
        if isinstance(value, bool):
            return value
        if isinstance(value, str):
            lowered = value.strip().lower()
            if lowered in ('true', '1', 'yes', 'on'):
                return True
            if lowered in ('false', '0', 'no', 'off'):
                return False
        if isinstance(value, (int, float)):
            return bool(value)
        return fallback

    @staticmethod
    def _as_int(value, fallback, minimum, maximum):
        """Coerce to an int and clamp it into range."""
        try:
            number = int(value)
        except (TypeError, ValueError):
            return fallback
        return max(minimum, min(maximum, number))

    @staticmethod
    def _as_audio_quality(value, fallback):
        """Normalise yt-dlp's preferredquality to a plain digit string.

        It is either a VBR level (0-10) or a kbps bitrate, and it ends up in
        an ffmpeg argument, so rebuild it from an int rather than passing
        whatever the caller sent.
        """
        try:
            quality = int(str(value).strip())
        except (TypeError, ValueError):
            return fallback
        if 0 <= quality <= 320:
            return str(quality)
        return fallback

    @staticmethod
    def _as_lang_list(value, fallback):
        """Sanitise the comma-separated subtitle languages.

        This string is handed to yt-dlp's subtitleslangs as-is, so restrict it
        to language-tag characters instead of trusting the caller.
        """
        if not isinstance(value, str):
            return fallback
        langs = []
        for lang in value.split(','):
            lang = lang.strip()
            if not lang or lang in langs:
                continue
            if all(c.isascii() and (c.isalnum() or c in '-_.*') for c in lang):
                langs.append(lang)
        # An emptied box would silently mean "no languages", which makes the
        # subtitles toggle do nothing — keep the last usable value instead.
        return ','.join(langs) if langs else fallback

    def _coerce(self, key, value, fallback):
        """Validate one setting, returning fallback when it is unusable."""
        if key == 'concurrent_fragments':
            return self._as_int(value, fallback, 1, self.MAX_CONCURRENT_FRAGMENTS)
        if key == 'rate_limit_kbps':
            return self._as_int(value, fallback, 0, self.MAX_RATE_LIMIT_KBPS)
        if key == 'container':
            return value if value in self.CONTAINERS else fallback
        if key == 'audio_format':
            return value if value in self.AUDIO_FORMATS else fallback
        if key == 'audio_quality':
            return self._as_audio_quality(value, fallback)
        if key == 'subtitle_langs':
            return self._as_lang_list(value, fallback)
        if key == 'sponsorblock_categories':
            if not isinstance(value, list):
                return list(fallback)
            # Unknown categories would make yt-dlp reject the whole run
            cleaned = []
            for category in value:
                if category in self.SPONSORBLOCK_CATEGORIES and category not in cleaned:
                    cleaned.append(category)
            return cleaned
        # Everything else in the contract is a checkbox
        return self._as_bool(value, fallback)

    @staticmethod
    def _settle(values):
        """Resolve the settings whose real meaning depends on another setting.

        SponsorBlock with every category unticked is not "on, with nothing to
        remove". apply_media_opts has to treat an empty category list as off,
        because yt-dlp reads one as *every* category (SponsorBlockPP.__init__,
        yt_dlp/postprocessor/sponsorblock.py:36) and would cut far more than was
        asked for — so the switch in that state does nothing at all, and does it
        silently. Storing it as off is simply what the app is going to do, and
        since the settings panel reads its state back from here, it is also what
        the switch will show: the no-op state cannot be reached or persisted.

        The cost is that re-ticking a category does not turn SponsorBlock back
        on by itself. That is the right way round — the switch went off because
        it had stopped meaning anything, and turning it on again should be the
        user's decision rather than a side effect of a checkbox.
        """
        if not values['sponsorblock_categories']:
            values['sponsorblock_enabled'] = False
        return values

    def _normalise(self, raw, base=None):
        """Build the full settings set from raw values, validating each one."""
        if base is None:
            base = self.DEFAULTS
        return self._settle({key: self._coerce(key, raw.get(key, base[key]), base[key])
                             for key in self.DEFAULTS})

    def get_settings(self):
        """Return the full settings contract with defaults applied."""
        return self._normalise(self._config)

    def save_settings(self, settings):
        """Merge a partial settings object in and persist it.

        Unknown keys are ignored, and a value that fails validation keeps
        whatever was stored before rather than snapping back to the default.
        """
        current = self.get_settings()
        if not isinstance(settings, dict):
            return current
        incoming = {key: settings[key] for key in self.DEFAULTS if key in settings}
        merged = self._normalise({**current, **incoming}, base=current)
        self._config.update(merged)
        self._save_config()
        return merged


# Global AppSettings instance
settings_manager = AppSettings()


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
                with urlopen(req, timeout=120, context=get_ssl_context()) as response:
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


# Hosts an update may be downloaded from. Anything else is refused so a
# rogue page in the webview can't use this endpoint to fetch arbitrary files.
ALLOWED_UPDATE_HOSTS = ('github.com', 'objects.githubusercontent.com', 'nightly.link')


@app.route('/api/update/download', methods=['GET'])
def update_download():
    """Download an update with SSE progress streaming."""
    asset_url = request.args.get('url')
    asset_name = request.args.get('name')

    if not asset_url or not asset_name:
        return jsonify({'error': 'Missing url or name parameters'}), 400

    parsed_url = urlparse(asset_url)
    if parsed_url.scheme != 'https' or parsed_url.hostname not in ALLOWED_UPDATE_HOSTS:
        return jsonify({'error': 'Untrusted update host'}), 400

    # Strip any directory component — the file belongs in the updates dir
    asset_name = os.path.basename(asset_name)
    if not asset_name or asset_name in ('.', '..'):
        return jsonify({'error': 'Invalid asset name'}), 400

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
                # Report the actual reason (TLS, 404, disk) — not just "failed"
                reason = update_manager.last_download_error or 'unknown error'
                yield f"data: {json.dumps({'percent': 0, 'status': f'Download failed: {reason}', 'success': False})}\n\n"

        except Exception as e:
            yield f"data: {json.dumps({'percent': 0, 'status': f'Error: {str(e)}', 'success': False})}\n\n"

    return Response(generate(), mimetype='text/event-stream')


@app.route('/api/update/apply', methods=['POST'])
def update_apply():
    """Run a downloaded installer, then exit so it can replace this app."""
    data = request.get_json(silent=True) or {}
    downloaded_path = data.get('path')

    if not downloaded_path or not isinstance(downloaded_path, str):
        return jsonify({'success': False, 'error': 'Downloaded file not found'})

    # Applying an update ends with os._exit(0) so the installer can replace the
    # files this process holds open. A download in flight dies with it, part way
    # through a write, leaving a .part file and no explanation. The update can
    # wait; the 2 GB already transferred cannot. (Defined further down the file,
    # next to the download endpoints that own it.)
    if is_download_running():
        return jsonify({'success': False, 'error': (
            'A download is still running. Installing an update closes '
            'FinFetcher, which would abandon it part way through — wait for it '
            'to finish, or cancel it, then install the update.')})

    # Every guard — frozen, inside the updates folder, actually the installer,
    # and whether Setup has already given up or stopped on a dialog — lives in
    # apply_update, next to the line that runs the file.
    started, message = update_manager.apply_update(downloaded_path)

    if not started:
        # Nothing was launched, or it launched and immediately stopped. Either
        # way this app is still the only one there is, so report why and stay
        # running rather than exiting into nothing.
        return jsonify({'success': False, 'error': message or 'Failed to start the installer'})

    # success here means the installer is running, not that the update worked:
    # this process is about to disappear and will never learn the outcome.
    # message is that distinction in the words the page should show, and
    # log_path is the only place the answer will exist afterwards.
    def shutdown():
        time.sleep(1)
        os._exit(0)
    threading.Thread(target=shutdown, daemon=True).start()
    return jsonify({'success': True, 'message': message,
                    'log_path': UpdateManager.get_update_log_path()})


@app.route('/api/update/settings', methods=['GET', 'POST'])
def update_settings():
    """Get or set update preferences."""
    if request.method == 'GET':
        settings = update_manager.get_settings()
        settings['current_version'] = update_manager.get_current_version()
        # Self-update only works for the packaged exe — the UI hides the
        # install buttons when running from source.
        settings['can_self_update'] = bool(getattr(sys, 'frozen', False))
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
        with urlopen(req, timeout=15, context=get_ssl_context()) as response:
            releases = json.loads(response.read().decode('utf-8'))

        result = []
        for release in releases:
            is_draft = release.get('draft', False)
            is_prerelease = release.get('prerelease', False)

            if is_draft:
                continue
            if is_prerelease and not include_prerelease:
                continue

            # Skip versions older than the minimum (no updater support)
            tag = release.get('tag_name', '')
            parsed = update_manager._parse_version(tag)
            if parsed[:3] < update_manager.MIN_UPDATE_VERSION:
                continue

            version = tag.lstrip('v')

            # Find the asset to offer — the installer when there is one
            picked = update_manager._pick_exe_asset(release.get('assets', []))
            exe_asset = {
                'name': picked['name'],
                'url': picked['browser_download_url'],
                'size': picked['size'],
                # False means self-update will refuse it: a bare exe from
                # 1.2.4 or earlier cannot replace an installed copy.
                'is_installer': update_manager._is_installer_name(picked['name']),
            } if picked else None

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


@app.route('/api/update/artifacts', methods=['GET'])
def update_artifacts():
    """List recent CI build artifacts from GitHub Actions.
    
    Uses the public GitHub API to list successful workflow runs,
    and constructs nightly.link URLs for unauthenticated download.
    """
    current_version = update_manager.get_current_version()
    try:
        # Fetch recent successful runs from the build-test workflow
        api_url = (
            'https://api.github.com/repos/mkiera/FinFetcher/actions/runs'
            '?status=success&per_page=20'
        )
        req = Request(api_url, headers={
            'User-Agent': 'FinFetcher-Updater/1.0',
            'Accept': 'application/vnd.github.v3+json',
        })
        with urlopen(req, timeout=15, context=get_ssl_context()) as response:
            data = json.loads(response.read().decode('utf-8'))

        result = []
        seen_branches = set()
        for run in data.get('workflow_runs', []):
            branch = run.get('head_branch', '')
            run_id = run.get('id')
            sha = run.get('head_sha', '')[:7]
            workflow_name = run.get('name', '')

            # Only include build-test runs (not release builds)
            if workflow_name != 'Build Test':
                continue

            # Show only the latest run per branch
            if branch in seen_branches:
                continue
            seen_branches.add(branch)

            # Construct nightly.link download URL
            # Format: https://nightly.link/owner/repo/actions/runs/{run_id}/{artifact_name}.zip
            # The artifact name from build-test.yml is normally FinFetcher_{branch_suffix},
            # but a workflow_dispatch run names it after the build_name input —
            # so ask GitHub for the real name and only guess if that fails.
            branch_suffix = branch
            for prefix in ('feature/', 'bugfix/'):
                if branch_suffix.startswith(prefix):
                    branch_suffix = branch_suffix[len(prefix):]
                    break
            artifact_name = f'FinFetcher_{branch_suffix}'
            try:
                art_req = Request(
                    f'https://api.github.com/repos/mkiera/FinFetcher/actions/runs/{run_id}/artifacts',
                    headers={
                        'User-Agent': 'FinFetcher-Updater/1.0',
                        'Accept': 'application/vnd.github.v3+json',
                    })
                with urlopen(art_req, timeout=5, context=get_ssl_context()) as art_resp:
                    artifacts = json.loads(art_resp.read().decode('utf-8')).get('artifacts', [])
                artifacts = [a for a in artifacts if not a.get('expired')]
                if artifacts:
                    artifact_name = artifacts[0].get('name', artifact_name)
            except Exception:
                pass
            download_url = f'https://nightly.link/mkiera/FinFetcher/actions/runs/{run_id}/{artifact_name}.zip'

            # Fetch version.txt from this commit
            version = ''
            try:
                ver_url = f'https://raw.githubusercontent.com/mkiera/FinFetcher/{run.get("head_sha", "")}/version.txt'
                ver_req = Request(ver_url, headers={'User-Agent': 'FinFetcher-Updater/1.0'})
                with urlopen(ver_req, timeout=5, context=get_ssl_context()) as ver_resp:
                    version = ver_resp.read().decode('utf-8').strip()
            except Exception:
                pass

            result.append({
                'branch': branch,
                'sha': sha,
                'version': version,
                'run_id': run_id,
                'artifact_name': artifact_name,
                'published_at': run.get('created_at', ''),
                'html_url': run.get('html_url', ''),
                'exe_asset': {
                    'name': f'{artifact_name}.zip',
                    'url': download_url,
                    'size': 0,  # Unknown until download
                },
            })

        return jsonify({
            'artifacts': result,
            'current_version': current_version,
        })

    except Exception as e:
        return jsonify({'error': str(e), 'artifacts': [], 'current_version': current_version})


# ============ Settings API Endpoints ============

@app.route('/api/settings', methods=['GET', 'POST'])
def app_settings():
    """Get or set the download preferences."""
    if request.method == 'GET':
        return jsonify(settings_manager.get_settings())
    # A partial object is the normal case — the UI posts only what changed.
    data = request.get_json(silent=True)
    if not isinstance(data, dict):
        # Reporting success for a write that never happened is how a setting
        # silently fails to stick.
        return jsonify({'success': False, 'error': 'Expected a JSON object'}), 400
    settings = settings_manager.save_settings(data)
    return jsonify({'success': True, 'settings': settings})


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
        ydl.cookiejar.save({cookies_file!r}, ignore_discard=True, ignore_expires=True)
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

def _try_browser_cookies(browser, result_holder, timeout=10):
    """Try to load cookies from a browser with a strict timeout.
    
    Uses a background thread to prevent yt-dlp from hanging the main thread
    if browser cookie extraction stalls (e.g., locked DB, missing browser).
    """
    def _probe():
        try:
            test_opts = {
                'cookiesfrombrowser': (browser,),
                'quiet': True,
                'no_warnings': True,
                'extract_flat': True,
            }
            with yt_dlp.YoutubeDL(test_opts) as ydl:
                # Just initialize the cookie jar — don't make a network request.
                # If the browser cookies can be read, the YoutubeDL context
                # manager succeeds. A full extract_info call can hang forever.
                if ydl.cookiejar is not None:
                    result_holder.append(browser)
        except Exception:
            pass

    t = threading.Thread(target=_probe, daemon=True)
    t.start()
    t.join(timeout=timeout)
    # If the thread is still alive after timeout, we abandon it (daemon=True)
    return len(result_holder) > 0


def get_cookie_opts():
    """Get yt-dlp cookie options for YouTube authentication.
    
    Tries to extract cookies from an installed browser to bypass
    YouTube's bot detection (403 Forbidden errors).
    On Windows, Chromium cookies are extracted via a python.exe subprocess
    to work around DPAPI failures in pythonw.exe.
    Result is cached for the lifetime of the app.
    
    All browser probes use strict timeouts to prevent the app from hanging.
    """
    global _cookie_opts_cache
    if _cookie_opts_cache is not None:
        return _cookie_opts_cache
    
    # On Windows, Chromium browsers need subprocess extraction due to DPAPI
    chromium_browsers = ['chrome', 'edge', 'brave', 'opera']
    non_chromium = ['firefox']
    
    if os.name == 'nt':
        # Try non-Chromium first (no DPAPI issues)
        for browser in non_chromium:
            result = []
            if _try_browser_cookies(browser, result, timeout=10):
                _cookie_opts_cache = {'cookiesfrombrowser': (browser,)}
                return _cookie_opts_cache
        
        # Try Chromium browsers via subprocess extraction
        # Skip if running as a frozen exe — _get_python_exe() would return
        # the app exe itself, which can't run arbitrary Python scripts.
        if not getattr(sys, 'frozen', False):
            for browser in chromium_browsers:
                cookies_file = _extract_cookies_via_subprocess(browser)
                if cookies_file:
                    _cookie_opts_cache = {'cookiefile': cookies_file}
                    return _cookie_opts_cache
        else:
            # Frozen exe: try Chromium directly (may fail due to DPAPI,
            # but won't hang thanks to the timeout wrapper)
            for browser in chromium_browsers:
                result = []
                if _try_browser_cookies(browser, result, timeout=10):
                    _cookie_opts_cache = {'cookiesfrombrowser': (browser,)}
                    return _cookie_opts_cache
    else:
        # Non-Windows: try all browsers directly with timeout
        for browser in chromium_browsers + non_chromium:
            result = []
            if _try_browser_cookies(browser, result, timeout=10):
                _cookie_opts_cache = {'cookiesfrombrowser': (browser,)}
                return _cookie_opts_cache
    
    # No browser cookies available — proceed without them
    _cookie_opts_cache = {}
    return _cookie_opts_cache


def get_video_info(url, flat=True):
    """Fetch video metadata using yt-dlp Python API.
    
    Uses a background thread with a 30-second timeout to prevent
    the app from hanging if yt-dlp stalls on network requests.
    """
    ydl_opts = {
        'quiet': True,
        'no_warnings': True,
        'extract_flat': flat,
    }
    ydl_opts.update(get_cookie_opts())
    
    result_holder = [None]
    error_holder = [None]
    
    def _fetch():
        try:
            with yt_dlp.YoutubeDL(ydl_opts) as ydl:
                result_holder[0] = ydl.extract_info(url, download=False)
        except Exception as e:
            error_holder[0] = e
    
    t = threading.Thread(target=_fetch, daemon=True)
    t.start()
    t.join(timeout=30)
    
    if t.is_alive():
        raise Exception("Request timed out after 30 seconds. YouTube may be blocking requests — try again or check your network connection.")
    
    if error_holder[0]:
        raise Exception(str(error_holder[0]))
    
    if result_holder[0] is None:
        raise Exception("No result returned from yt-dlp.")
    
    return result_holder[0]


def estimate_size(formats, quality='max'):
    """Estimate download size from format info."""
    if not formats:
        return None
    
    # Try to find best matching format
    best_video = None
    best_audio = None
    
    for f in formats:
        # yt-dlp uses the string 'none' (not None) for a missing stream
        has_video = f.get('vcodec') not in (None, 'none')
        has_audio = f.get('acodec') not in (None, 'none')
        if has_video:
            if not best_video or (f.get('height', 0) or 0) > (best_video.get('height', 0) or 0):
                best_video = f
        if has_audio and not has_video:
            if not best_audio or (f.get('abr', 0) or 0) > (best_audio.get('abr', 0) or 0):
                best_audio = f

    total = 0
    if best_video:
        total += best_video.get('filesize') or best_video.get('filesize_approx') or 0
    if best_audio:
        total += best_audio.get('filesize') or best_audio.get('filesize_approx') or 0
    
    return total if total > 0 else None


def parse_timestamp(value):
    """Parse 'SS', 'MM:SS' or 'HH:MM:SS' into seconds. None if unparseable."""
    if not value:
        return None
    try:
        parts = [int(p) for p in str(value).strip().split(':')]
    except ValueError:
        return None
    if not 1 <= len(parts) <= 3 or any(p < 0 for p in parts):
        return None
    seconds = 0
    for part in parts:
        seconds = seconds * 60 + part
    return seconds


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

    # Which CA store HTTPS verification uses — a missing certifi bundle
    # breaks downloads on machines whose OS store holds an expired root
    debug_info['dependencies']['CA store'] = get_ca_source()
    
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
    
    result_holder = [None]
    
    def _run():
        try:
            ydl_opts = {'quiet': True}
            ydl_opts.update(get_cookie_opts())
            with yt_dlp.YoutubeDL(ydl_opts) as ydl:
                info = ydl.extract_info(test_url, download=False)
                result_holder[0] = {
                    'success': True,
                    'message': 'yt-dlp can fetch video info successfully!',
                    'title': info.get('title', 'Unknown')
                }
        except Exception as e:
            result_holder[0] = {
                'success': False,
                'message': 'Exception',
                'error': str(e)
            }
    
    t = threading.Thread(target=_run, daemon=True)
    t.start()
    t.join(timeout=30)
    
    if t.is_alive():
        return jsonify({
            'success': False,
            'message': 'Timeout',
            'error': 'Diagnostic test timed out after 30 seconds. This usually means yt-dlp is hanging on a network request or cookie extraction.'
        })
    
    if result_holder[0]:
        return jsonify(result_holder[0])
    
    return jsonify({
        'success': False,
        'message': 'Unknown',
        'error': 'No result returned from diagnostic test.'
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


def apply_performance_opts(ydl_opts, settings, download_type, save_path):
    """Fold the performance settings into a yt-dlp options dict, in place.

    Split out of download() so the option assembly stays readable as more
    settings get wired in.
    """
    fragments = settings['concurrent_fragments']

    # The UI collects KB/s; yt-dlp wants bytes/sec. A present-but-zero
    # ratelimit is worse than no limit — yt-dlp divides by it — so an
    # unlimited setting has to leave the key out entirely.
    rate_limit_kbps = settings['rate_limit_kbps']
    if rate_limit_kbps > 0:
        # Exactly what the user asked for. Dividing it by the fragment count
        # made the cap that many times too strict on the common case: a
        # progressive download is a single stream, and concurrent fragments
        # only exist for native HLS/DASH.
        ydl_opts['ratelimit'] = max(1, rate_limit_kbps * 1024)
        # FileDownloader.slow_down throttles the transfer it is called from,
        # using that transfer's own byte counter, so N fragments in flight
        # would each be allowed the full limit and the total would run at N
        # times the cap. One fragment at a time is what makes the typed
        # number the real ceiling in the fragmented case too; a progressive
        # download has one stream regardless, so nothing is lost there.
        fragments = 1

    ydl_opts['concurrent_fragment_downloads'] = fragments

    # The archive lets a re-run of a playlist skip what it already fetched.
    # Never for a single video: there the user asked for this one file, and
    # a second attempt would silently do nothing.
    #
    # It lives beside the files it describes rather than in one global list,
    # because the archive key is only extractor+id: a shared file would make
    # a playlist already saved to one folder look "done" for every other
    # folder, mode and quality too.
    if settings['use_download_archive'] and download_type != 'single':
        ydl_opts['download_archive'] = os.path.join(save_path, '.finfetcher-archive.txt')


# Every extension EmbedThumbnailPP can actually put a cover image into, read
# off the chain of `info['ext']` tests in its run() (yt_dlp/postprocessor/
# embedthumbnail.py:90-220 in 2026.02.04). It has no SUPPORTED_EXTS constant to
# ask; that chain ends in a bare `else: raise EmbedThumbnailPPError`, and
# YoutubeDL turns a PostProcessingError into report_error (YoutubeDL.py:
# 3622-3625), so an unsupported container does not lose the cover image — it
# fails a download whose file is already written and perfectly good.
#
# ogg, opus and flac are handled exclusively through mutagen, so they belong
# here only when that import actually resolved; without it the post-processor
# raises for those three as well.
def _embeddable_thumbnail_exts():
    exts = ['mp3', 'mkv', 'mka', 'm4a', 'mp4', 'm4v', 'mov']
    try:
        from yt_dlp.dependencies import mutagen as _mutagen
    except Exception:
        _mutagen = None
    if _mutagen is not None:
        exts += ['ogg', 'opus', 'flac']
    return tuple(exts)


EMBEDDABLE_THUMBNAIL_EXTS = _embeddable_thumbnail_exts()
# FFmpegEmbedSubtitlePP.SUPPORTED_EXTS, minus the ones this app never produces.
# Belt and braces rather than load-bearing: that post-processor checks the
# extension itself and skips with a message (ffmpeg.py:590-592), which is
# exactly what EmbedThumbnailPP does not do.
EMBEDDABLE_SUBTITLE_EXTS = ('mp4', 'mkv', 'webm', 'm4a')


class FinFetcherThumbnailGuardPP(PostProcessor):
    """Take the thumbnail away from EmbedThumbnail when the file cannot hold it.

    Which container the file ends up in is not knowable when the options are
    built. In audio mode it is — FFmpegExtractAudio forces the extension to the
    chosen codec (yt_dlp/postprocessor/ffmpeg.py:528) — but a video download's
    format selector ends in a bare "best", and if that is the branch that
    matches, what arrives is a single progressive stream that keeps its own
    extension: merge_output_format only ever applies to a merge (YoutubeDL.py:
    3446-3459). So a request for mp4 can perfectly well produce a .webm, and
    that combination used to fail the whole download at the last step, after
    the file was on disk and correct.

    yt-dlp has half of this itself — a merged webm is switched to mkv when an
    EmbedThumbnailPP is registered (YoutubeDL.py:3450-3457) — but only for a
    merge, and only when no merge_output_format was given, which is never the
    case here. So the check has to happen with the file in hand, and this is
    where: run in front of EmbedThumbnail, it empties the thumbnail list, which
    is the quiet early exit that post-processor does offer (embedthumbnail.py:
    61-68). Nothing raises, and the download stands.
    """

    def run(self, info):
        ext = info.get('ext')
        if ext in EMBEDDABLE_THUMBNAIL_EXTS:
            return [], info

        stranded = [thumb.get('filepath') for thumb in info.get('thumbnails') or []]
        info['thumbnails'] = []
        self.to_screen(f'{ext} cannot hold a cover image, so the thumbnail '
                       'was not embedded')
        # The image was only fetched in order to be embedded — writethumbnail is
        # set for that and nothing else — so leaving it in the save folder would
        # be litter the user never asked for. info= so its entry in
        # __files_to_move goes with it and yt-dlp does not chase a deleted file.
        self._delete_downloaded_files(*stranded, info=info)
        return [], info


def _register_thumbnail_guard():
    """Make the guard usable as a `postprocessors` entry, and prove that it is.

    yt-dlp resolves every {'key': ...} entry through
    yt_dlp.postprocessor.get_postprocessor, which is a lookup in a plain dict of
    registered post-processors — the same dict its own plugin loader writes into
    (yt_dlp/postprocessor/__init__.py). Registering one class there is therefore
    the intended shape of this, and it is what keeps the whole post-processor
    chain in one ordered list in apply_media_opts instead of half of it being
    bolted on afterwards, where it would land after FFmpegSplitChapters and the
    split pieces would lose their cover art.

    yt_dlp/globals.py says in as many words that the plugin/globals API carries
    no compatibility guarantee, so nothing here is assumed: get_postprocessor
    has to hand the class back before apply_media_opts will name it. Returns the
    key to use, or None to fall back to a bare EmbedThumbnail with only the
    container setting guarding it — the old behaviour.
    """
    try:
        from yt_dlp import postprocessor as ytdlp_pps

        registry = getattr(ytdlp_pps, 'postprocessors', None)
        # An older yt-dlp looked the name up in the module's own namespace
        registry = vars(ytdlp_pps) if registry is None else registry.value
        registry[FinFetcherThumbnailGuardPP.__name__] = FinFetcherThumbnailGuardPP

        key = FinFetcherThumbnailGuardPP.pp_key()
        if ytdlp_pps.get_postprocessor(key) is FinFetcherThumbnailGuardPP:
            return key
    except Exception:
        pass
    return None


THUMBNAIL_GUARD_KEY = _register_thumbnail_guard()


def _is_partial_download_refusal(error_payload):
    """True when a failed attempt failed *because* a range was requested.

    yt-dlp's two refusals are worded in YoutubeDL.py:3442-3443. Anything else
    (403, geo-block, dropped connection) is a real error and must not be
    retried as a full download.
    """
    message = (error_payload or {}).get('error') or ''
    return ('cannot be partially downloaded' in message
            or 'partially, but ffmpeg is not installed' in message)


def apply_media_opts(ydl_opts, settings, mode, save_path, fast_trim=False):
    """Fold the media settings into a yt-dlp options dict, in place.

    fast_trim means the file will contain only a slice of the source. Every
    timestamp yt-dlp carries — chapter marks, SponsorBlock segments — still
    refers to the full video, so anything that cuts or labels by those times
    is skipped for that run rather than applied at the wrong offsets.

    The post-processor chain is built in the same order yt-dlp's own CLI
    builds it (get_postprocessors in yt_dlp/__init__.py): SponsorBlock,
    FFmpegExtractAudio, FFmpegEmbedSubtitle, ModifyChapters, FFmpegMetadata,
    EmbedThumbnail (with this app's thumbnail guard directly in front of it),
    FFmpegSplitChapters. That order is load-bearing, not
    cosmetic — ModifyChapters physically cuts the file, so a cover image or a
    chapter list written before it would describe timings that no longer
    exist, and subtitles have to be inside the container before it cuts.

    Whatever the mode-specific block already put in ydl_opts (the audio
    extraction) keeps its own slot in that sequence.
    """
    # The extension the file is meant to end up with. Exact in audio mode, where
    # FFmpegExtractAudio forces it to the chosen codec; only a preference in
    # video mode, where a download that falls back to a single progressive
    # format keeps that format's own extension instead.
    target_ext = settings['audio_format'] if mode == 'audio' else settings['container']

    if settings['subtitles_enabled']:
        ydl_opts['writesubtitles'] = True
        ydl_opts['writeautomaticsub'] = settings['subtitles_auto']
        ydl_opts['subtitleslangs'] = [lang for lang in settings['subtitle_langs'].split(',') if lang]

    # SponsorBlock reads an empty category list as "every category", so an
    # enabled toggle with nothing ticked has to count as off. AppSettings._settle
    # keeps that state from being stored at all, so this is the guard for a
    # caller that assembled its settings by hand rather than a live case. On a
    # trimmed download its segment times address the untrimmed timeline, so it
    # would cut the wrong parts out of the clip.
    sponsor_categories = (settings['sponsorblock_categories']
                          if settings['sponsorblock_enabled'] and not fast_trim else [])

    # Same reasoning for chapters: the marks and the split points belong to
    # the full video, not to the slice that was actually downloaded.
    embed_chapters = settings['embed_chapters'] and not fast_trim
    split_chapters = settings['split_chapters'] and not fast_trim

    # Two checks, because they answer different questions. This one is about the
    # container the user asked for: a webm or a wav can never carry a cover, so
    # there is no sense fetching an image for one. What it cannot answer is what
    # the file will actually be — see FinFetcherThumbnailGuardPP, which is added
    # to the chain below and settles that with the file in hand.
    embed_thumbnail = settings['embed_thumbnail'] and target_ext in EMBEDDABLE_THUMBNAIL_EXTS
    if embed_thumbnail:
        # The image has to be on disk before EmbedThumbnail can read it
        ydl_opts['writethumbnail'] = True

    postprocessors = []
    if sponsor_categories:
        # Only tags the segments — ModifyChapters below is what cuts them out.
        # after_filter so the segments are known before anything writes chapters.
        postprocessors.append({
            'key': 'SponsorBlock',
            'categories': sponsor_categories,
            'when': 'after_filter',
        })
    postprocessors.extend(ydl_opts.get('postprocessors', []))
    if (settings['subtitles_enabled'] and settings['embed_subtitles']
            and target_ext in EMBEDDABLE_SUBTITLE_EXTS):
        postprocessors.append({
            'key': 'FFmpegEmbedSubtitle',
            # False lets the sidecar files go once they are in the container,
            # which is the difference between "embed" and "leave as sidecar"
            'already_have_subtitle': False,
        })
    if sponsor_categories:
        postprocessors.append({
            'key': 'ModifyChapters',
            'remove_sponsor_segments': sponsor_categories,
            # Not precise_trim: that setting is about where a trim cuts. Here
            # force_keyframes re-encodes the entire file so a keyframe lands on
            # each cut point (ModifyChapters.remove_chapters -> force_keyframes),
            # which turned every SponsorBlock removal into a full re-encode and
            # left the .keyframes.temp file behind. yt-dlp's own default is off.
            'force_keyframes': False,
        })
    if settings['embed_metadata'] or embed_chapters:
        postprocessors.append({
            'key': 'FFmpegMetadata',
            'add_metadata': settings['embed_metadata'],
            'add_chapters': embed_chapters,
            # This app never writes an .info.json, so there is none to attach
            'add_infojson': False,
        })
    if embed_thumbnail:
        if THUMBNAIL_GUARD_KEY:
            # Immediately in front, so it sees exactly the file EmbedThumbnail
            # is about to be handed
            postprocessors.append({'key': THUMBNAIL_GUARD_KEY})
        postprocessors.append({
            'key': 'EmbedThumbnail',
            # False deletes the downloaded image once it is embedded
            'already_have_thumbnail': False,
        })
    if split_chapters:
        postprocessors.append({
            'key': 'FFmpegSplitChapters',
            # Same as ModifyChapters above: this re-encodes the whole file to
            # put a keyframe at every chapter boundary, which is not what the
            # trim setting asked for. Off, as in yt-dlp.
            'force_keyframes': False,
        })

    if postprocessors:
        ydl_opts['postprocessors'] = postprocessors

    # outtmpl only has to become a dict when one of these secondary templates
    # is in play, so leave it the plain string it is today otherwise.
    extra_outtmpl = {}
    if embed_thumbnail:
        # A playlist's own thumbnail belongs to no file we embed into, so
        # writethumbnail would just strand an image in the download folder.
        extra_outtmpl['pl_thumbnail'] = ''
    if split_chapters:
        # yt-dlp's built-in chapter template carries no directory, so without
        # this the pieces land in the working directory, not the save folder.
        extra_outtmpl['chapter'] = os.path.join(
            save_path, '%(title)s - %(section_number)03d %(section_title)s.%(ext)s')
    if extra_outtmpl:
        ydl_opts['outtmpl'] = {'default': ydl_opts['outtmpl'], **extra_outtmpl}


def _scrap_name_roots(path, folder):
    """Reduce a path yt-dlp reported to the file name its scraps are named for.

    Returns (name, stem) for a path that lives directly in folder, or None.
    yt-dlp writes to "<name>.part", and a fragmented download writes
    "<name>.part-Frag7" (and "<name>.part-Frag7.part") beside it, so a
    reported in-progress path has to be wound back to the finished name
    before anything is matched against it.
    """
    if not isinstance(path, str) or not path:
        return None
    try:
        real = os.path.realpath(path)
    except Exception:
        return None
    # Only the save folder itself — never a subfolder, never anywhere else
    if os.path.dirname(real) != folder:
        return None

    name = os.path.basename(real)
    while name:
        if name.lower().endswith('.part'):
            name = name[:-len('.part')]
            continue
        marker = name.rfind('-Frag')
        if marker > 0 and name[marker + len('-Frag'):].isdigit():
            name = name[:marker]
            continue
        break
    if not name:
        return None
    return name, os.path.splitext(name)[0]


def _remove_file_with_retry(path, attempts=4, delay=0.5):
    """Delete a file a just-stopped ffmpeg may still be holding open.

    Windows refuses the unlink until the last handle is closed, and ffmpeg's
    can outlive the run — or the terminate() that ended it — by a moment, so
    one refusal means "not yet" rather than "impossible".

    Returns (removed, error). error is None when the file went away or was
    never there, so a caller can tell "nothing to do" from "still locked".
    """
    for attempt in range(attempts):
        try:
            os.remove(path)
            return True, None
        except FileNotFoundError:
            return False, None
        except OSError as e:
            if attempt == attempts - 1:
                return False, e
            time.sleep(delay)
    return False, None


def _terminate_process(proc):
    """Stop a child process and wait until it has actually gone.

    Returning while it is still dying is the bug this guards against: on
    Windows the file it was writing stays locked until the process exits, and
    the caller is usually about to delete that file.
    """
    if proc is None:
        return
    try:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)
    except Exception:
        # Already gone, or never really started. Either way there is nothing
        # left to stop, and no caller can do anything useful about it.
        pass


# The markers yt-dlp's post-processors splice in front of a file's extension
# via prepend_extension, giving "<stem>.<marker>.<ext>" beside the real file.
# Each one is renamed away or deleted on a clean run, so any that survive
# belong to a run that was cancelled or failed part way through:
#   temp            the working copy nearly every ffmpeg post-processor writes
#                   (ExtractAudio, EmbedSubtitle, Metadata, EmbedThumbnail,
#                   Fixup, ModifyChapters — yt_dlp/postprocessor/ffmpeg.py and
#                   modify_chapters.py:315)
#   keyframes.temp  the keyframe re-encode FFmpegPostProcessor.force_keyframes
#                   makes (ffmpeg.py:389-397)
#   uncut           the original ModifyChapters sets aside before it swaps the
#                   cut version into place (modify_chapters.py:69-73)
#   orig            the pre-conversion audio ExtractAudio sets aside when the
#                   output keeps the same name (ffmpeg.py:515-516)
POSTPROCESSOR_SCRAP_MARKERS = ('temp', 'keyframes.temp', 'uncut', 'orig')


def _cleanup_download_scraps(save_path, seen_files, cancelled=False, since=None):
    """Delete the temp files a finished or failed download can leave behind.

    Two kinds, both named after the file that was being written:
      - "<stem>.<marker>.<ext>" for every marker in
        POSTPROCESSOR_SCRAP_MARKERS. yt-dlp cleans these up itself on the way
        out, but a cancel stops the chain between stages, and on Windows even
        its own delete fails while ffmpeg still holds the handle.
      - "<name>.part" and its "-FragN" pieces, the in-progress files
        FileDownloader.temp_name gives a download (yt_dlp/downloader/
        common.py:217-222).

    Only names derived from a path this run actually reported are considered,
    and only files sitting directly in save_path: a scrap that cannot be
    attributed to this download is somebody else's file and is left alone.
    The files the download produced are skipped too, in case a video is
    genuinely titled like one of these temp names.

    since (a time.time() from before the download began) is the last of those
    guards: a file older than the run cannot have come from it, whatever it is
    called. "Video.orig.mp4" is a name a person might well have chosen, and it
    only becomes ours if this run is what wrote it. Two seconds of slack because
    FAT-family filesystems store mtimes to that resolution.

    cancelled widens that last exemption: a run stopped part-way through
    post-processing leaves its output under the name the finished file would
    have had (yt-dlp renames off ".part" before the post-processors run), and
    that would sit in the folder passing for a completed download. The caller
    only sets this when the output really is part-written — see
    finish_cancelled in download().

    Returns log lines for anything that could not be removed.
    """
    messages = []
    try:
        folder = os.path.realpath(save_path)
    except Exception:
        return messages

    names, stems, produced = set(), set(), set()
    for path in list(seen_files):
        roots = _scrap_name_roots(path, folder)
        if roots is None:
            continue
        name, stem = roots
        names.add(name)
        stems.add(stem)
        produced.add(os.path.join(folder, name))

    if not names:
        return messages

    try:
        entries = os.listdir(folder)
    except OSError:
        return messages

    removed = []
    for entry in entries:
        target = os.path.join(folder, entry)
        if not os.path.isfile(target):
            continue
        if since is not None:
            try:
                if os.path.getmtime(target) < since - 2:
                    continue  # predates this download, so it is not ours
            except OSError:
                continue
        if target in produced:
            if not cancelled:
                continue
        else:
            is_scrap = (any(entry.startswith(f'{name}.part') for name in names)
                        or any(entry.startswith(f'{stem}.{marker}.')
                               for stem in stems
                               for marker in POSTPROCESSOR_SCRAP_MARKERS))
            if not is_scrap:
                continue

        gone, error = _remove_file_with_retry(target)
        if gone:
            removed.append(entry)
        elif error is not None:
            messages.append(
                f'> [FinFetcher] Could not remove leftover file {entry}: {error}')

    if removed:
        label = ('Removed the partial download'
                 if cancelled else 'Cleaned up leftover files')
        messages.insert(0, f'> [FinFetcher] {label}: ' + ', '.join(sorted(removed)))
    return messages


class DownloadJob:
    """One download in flight, plus the handles needed to stop it.

    Cancel arrives on the Flask thread serving /api/download/cancel, while the
    work is happening on the download thread and inside a child process, so
    both handles have to live somewhere all three can reach.

    yt-dlp itself only needs the flag for a plain download: raising
    DownloadCancelled from a hook stops it, because YoutubeDL re-raises that
    exception rather than retrying it (_handle_extraction_exceptions,
    YoutubeDL.py:1699) and lets it out of download() (__download_wrapper,
    :3648). ffmpeg is a separate process that never sees the flag, so its Popen
    is kept here to be killed directly — an ffmpeg left running is exactly what
    held the handle on the leftover file the user could not delete.

    More than one process can be tracked at a time because they come from two
    places: the trim this app spawns itself, and the one yt-dlp spawns for a
    ranged download (see _install_ytdlp_child_hook). Those two never overlap
    today, but a single slot would silently drop one if they ever did.
    """

    def __init__(self):
        self._cancelled = threading.Event()
        self._lock = threading.Lock()
        self._processes = []

    def is_cancelled(self):
        return self._cancelled.is_set()

    def cancel(self):
        """Ask this download to stop. Safe to call from any thread."""
        self._cancelled.set()
        self._stop_processes()

    def attach_process(self, proc):
        """Track a child process so a cancel can reach it.

        A cancel that landed while the process was being spawned has to take
        effect here as well, or that child would outlive the download that
        owns it — which is the whole point of tracking it.
        """
        if proc is None:
            return
        with self._lock:
            if proc not in self._processes:
                self._processes.append(proc)
        if self.is_cancelled():
            self._stop_processes()

    def detach_process(self, proc):
        """Stop tracking a process the caller is taking responsibility for."""
        with self._lock:
            if proc in self._processes:
                self._processes.remove(proc)

    def _stop_processes(self):
        with self._lock:
            processes = list(self._processes)
        for proc in processes:
            _terminate_process(proc)


# The download thread's claim on whatever yt-dlp spawns while it runs. Thread
# local rather than global because it is set on exactly the thread that calls
# into yt-dlp, so nothing else can accidentally adopt a stray process.
_ytdlp_child = threading.local()


def _ffmpeg_output_path(args):
    """The file an ffmpeg command line writes to, or None.

    FFmpegFD appends the destination last, through
    FFmpegPostProcessor._ffmpeg_filename_argument (yt_dlp/downloader/
    external.py:632), which prefixes a local path with "file:" so ffmpeg cannot
    mistake a drive letter for a protocol (yt_dlp/postprocessor/ffmpeg.py:
    370-377). That destination is the ".part" file, and killing ffmpeg is the
    only way this app ever learns of it: no progress hook fires for a download
    that never finished, so without this the cleanup would have no name to
    match and would leave the partial file behind.
    """
    if not isinstance(args, (list, tuple)) or not args:
        return None
    last = args[-1]
    if not isinstance(last, str) or last == '-':
        return None
    if last.startswith(('http://', 'https://')):
        return None  # a stream target, not a file of ours
    return last[len('file:'):] if last.startswith('file:') else last


def _install_ytdlp_child_hook():
    """Make the ffmpeg yt-dlp spawns for a ranged download reachable by cancel.

    A download with download_ranges set goes to FFmpegFD, whose _call_downloader
    starts ffmpeg and then sits in proc.wait() until it is done
    (yt_dlp/downloader/external.py:636-652). Nothing calls a progress hook in
    the meantime — ExternalFD.real_download only reports progress once, after
    the child has exited (external.py:60-73) — so the cancel flag those hooks
    check is never read, and Cancel did nothing at all until the whole range had
    been fetched. FFmpegFD.on_process_started, the one override point that is
    handed the child, is only called when the output is piped
    (external.py:636-638), which a download to a file never is. There is no
    supported hook.

    So the child is claimed where it is created. external.py binds Popen as a
    module-level name and calls it unqualified, so replacing that one name with
    a subclass registers every external-downloader process with the job running
    it. This is deliberately the narrowest reach available: a constructor
    signature is far more stable across yt-dlp versions than the body of
    _call_downloader would be. It is still a private symbol, so a failure to
    apply is caught and reported rather than assumed away — see the cancel
    message in run_attempt, which says so when this returns False.
    """
    try:
        from yt_dlp.downloader import external as ytdlp_external

        if getattr(ytdlp_external.Popen, '_finfetcher_tracked', False):
            # Already wrapped. A second wrapper would work but stack another
            # subclass on the first, and main.pyw is imported more than once
            # per process by the test harness.
            return True

        class _JobTrackedPopen(ytdlp_external.Popen):
            """yt-dlp's Popen, handed to the download that caused it."""

            _finfetcher_tracked = True

            def __init__(self, *args, **kwargs):
                super().__init__(*args, **kwargs)
                job = getattr(_ytdlp_child, 'job', None)
                if job is not None:
                    job.attach_process(self)
                remember = getattr(_ytdlp_child, 'remember', None)
                if remember is not None:
                    # yt-dlp always passes the command line positionally; the
                    # keyword form is only here so a caller that does not
                    # cannot raise out of a download.
                    command = args[0] if args else kwargs.get('args')
                    remember(_ffmpeg_output_path(command))

        ytdlp_external.Popen = _JobTrackedPopen
        return True
    except Exception:
        return False


# False means a ranged download cannot be interrupted, and the UI has to say so
# instead of pretending the Cancel button did something.
FFMPEG_DOWNLOAD_CANCELLABLE = _install_ytdlp_child_hook()


# This app runs one download at a time, so the cancel endpoint only needs to
# know about one job. A real queue would need an id per download and a way for
# the UI to name them; that is a later phase, not what a Cancel button needs.
_download_job_lock = threading.Lock()
_current_download_job = None


def _set_current_download_job(job):
    """Make job the download that /api/download/cancel acts on."""
    global _current_download_job
    with _download_job_lock:
        _current_download_job = job


def _clear_current_download_job(job):
    """Forget job once its stream is over.

    Identity-checked: if a newer download has already registered itself,
    clearing unconditionally would leave the live one with no cancel path.
    """
    global _current_download_job
    with _download_job_lock:
        if _current_download_job is job:
            _current_download_job = None


def is_download_running():
    """True while a download is registered, i.e. anything is still being written.

    The same registration the Cancel button acts on, so the answer is exactly
    as accurate as cancelling is. Used by the update endpoint, which would
    otherwise exit the process out from under a running download.
    """
    with _download_job_lock:
        return _current_download_job is not None


@app.route('/api/download/cancel', methods=['POST'])
def cancel_download():
    """Stop the running download, if there is one.

    Answers 200 either way: "nothing is running" is an answer, not a failed
    request. The download's own SSE stream is what reports the cancellation to
    the user, so this only has to say whether it found something to stop.
    """
    with _download_job_lock:
        job = _current_download_job
    if job is None:
        return jsonify({'success': False, 'error': 'No download is running'})
    job.cancel()
    return jsonify({'success': True})


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

    # Anything in the save folder older than this belongs to somebody else, so
    # the leftover cleanup will not touch it however it is named.
    run_started = time.time()

    if not url:
        return jsonify({'error': 'No URL provided'}), 400

    # Set save path (default to Downloads folder)
    if not save_path:
        save_path = os.path.join(os.path.expanduser("~"), "Downloads")
    if not os.path.exists(save_path):
        os.makedirs(save_path)

    # Output template — the artist prefix (and its separator) is dropped
    # entirely when the video has no artist metadata, instead of "NA - ".
    output_template = '%(title)s.%(ext)s' if mode == 'video' else '%(artist&{} - |)s%(title)s.%(ext)s'
    
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

    # Saved preferences from the Settings panel
    settings = settings_manager.get_settings()
    apply_performance_opts(ydl_opts, settings, download_type, save_path)

    # Trim planning. parse_timestamp() is the one parser for these values —
    # it already rejects negatives and malformed input.
    trim_requested = bool(trim_start and trim_end)
    start_sec, end_sec = parse_timestamp(trim_start), parse_timestamp(trim_end)
    trim_range_ok = (start_sec is not None and end_sec is not None
                     and end_sec > start_sec)
    # Playlists are excluded because there is no single file to cut; a stale
    # checkbox used to re-encode whichever entry finished last.
    fast_trim = trim_requested and trim_range_ok and download_type == 'single'

    if fast_trim:
        # Ask the extractor for just the requested range instead of pulling
        # the whole video and re-encoding it afterwards. Sources that cannot
        # serve a partial download fall back to the ffmpeg trim below.
        ydl_opts['download_ranges'] = download_range_func(None, [(start_sec, end_sec)])
        ydl_opts['force_keyframes_at_cuts'] = settings['precise_trim']

    # Mode-specific options
    if mode == 'audio':
        ydl_opts.update({
            'format': 'bestaudio/best',
            'postprocessors': [{
                'key': 'FFmpegExtractAudio',
                'preferredcodec': settings['audio_format'],
                'preferredquality': settings['audio_quality'],
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
        
        # webm cannot hold H.264/AAC, and yt-dlp will still try to mux into
        # it rather than pick something workable, failing the download. The
        # "/mkv" fallback gives it somewhere to land. mp4 needs no fallback —
        # it muxes vp9+opus fine — and adding one would silently turn mp4
        # downloads into mkv.
        container = settings['container']
        merge_format = 'webm/mkv' if container == 'webm' else container

        ydl_opts.update({
            'format': format_spec,
            'merge_output_format': merge_format,
        })

    # Post-processors last: the chain has to wrap whichever extraction the
    # mode block just chose.
    apply_media_opts(ydl_opts, settings, mode, save_path, fast_trim=fast_trim)

    # Queues for communicating with thread
    msg_queue = queue.Queue()
    result_queue = queue.Queue()

    # Every path this run reports, so the cleanup at the end only ever touches
    # scraps it can trace back to this download. Written from the download
    # thread, read once it has finished.
    seen_files = set()

    def remember_file(path):
        """Note a path yt-dlp reported, for the leftover cleanup."""
        if isinstance(path, str) and path:
            seen_files.add(path)

    # Registered below, before the response is handed back, so a Cancel that
    # lands the instant the download starts still finds this run.
    job = DownloadJob()

    def progress_hook(d):
        """Callback for yt-dlp progress."""
        remember_file(d.get('filename'))
        remember_file(d.get('tmpfilename'))
        # The one place a running download can be interrupted from: this fires
        # on every chunk, and DownloadCancelled is the exception YoutubeDL
        # treats as "stop" rather than as an error worth retrying. Raised
        # after the paths are recorded so the cleanup still knows about them.
        if job.is_cancelled():
            raise DownloadCancelled('Cancelled by the user')
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
        # Post-processing renames as it goes (extraction, merge, chapter cut),
        # so each stage's path is another name the scraps can be based on.
        final_path = d.get('info_dict', {}).get('filepath')
        remember_file(final_path)
        # Fires either side of every post-processor (PostProcessorMetaClass.
        # run_wrapper in yt_dlp/postprocessor/common.py:17-25), so cancelling
        # here stops the chain before the next stage starts. An ffmpeg step
        # already running inside yt-dlp is not ours to kill — it finishes, and
        # then nothing further runs.
        if job.is_cancelled():
            raise DownloadCancelled('Cancelled by the user')
        if d['status'] == 'finished':
            # Capture the final filepath after post-processing
            if final_path:
                msg_queue.put({'log': f"[postprocess] Final file: {final_path}"})
                result_queue.put({'final_file': final_path})
    
    ydl_opts['postprocessor_hooks'] = [postprocessor_hook]

    def run_download_thread(opts):
        # The ContextVar half of ffmpeg discovery is per-thread, and this is the
        # thread that asks yt-dlp to choose a downloader.
        ensure_ffmpeg_discoverable()
        # Anything yt-dlp spawns from here on belongs to this job, and the file
        # it writes to is a name the leftover cleanup needs. No teardown: each
        # attempt gets a thread of its own, so this claim dies with it.
        _ytdlp_child.job = job
        _ytdlp_child.remember = remember_file
        try:
            with yt_dlp.YoutubeDL(opts) as ydl:
                ydl.download([url])
            result_queue.put({'success': True})
        except Exception as e:
            # A cancel is not a failure to show the user as an error. It
            # normally arrives as the DownloadCancelled our own hook raised,
            # but an aborted step can surface as whatever it was in the middle
            # of, so the flag decides how this is reported, not the type.
            if job.is_cancelled() or isinstance(e, DownloadCancelled):
                result_queue.put({'success': False, 'cancelled': True})
            else:
                result_queue.put({'success': False, 'error': str(e)})

    # The threads the attempts run on. The download thread is a daemon and
    # keeps going if the stream is dropped, so the cleanup path needs a way to
    # ask whether anything is still writing.
    attempt_threads = []

    try:
        def run_attempt(opts):
            """Run one download to completion, streaming its logs as SSE.

            A sub-generator, so the caller can `yield from` it and still get a
            result back — the trim fallback has to know whether an attempt
            produced a file before it decides to try again. The failure
            payload is returned rather than yielded so a retried attempt does
            not report an error the user never needs to see.
            """
            # Cancelled before this attempt could start — nothing to run, and
            # starting a thread here would only give the cleanup a live writer
            # to race.
            if job.is_cancelled():
                return None, False, None

            # Start download thread
            t = threading.Thread(target=run_download_thread, args=(opts,), daemon=True)
            attempt_threads.append(t)
            t.start()

            final_file = None
            download_success = False
            error_payload = None
            cancel_logged = False

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

                # 2. Say so once when a cancel is being acted on. The hooks
                #    stop yt-dlp at the next chunk or post-processing stage,
                #    which is immediate mid-download but has to wait out an
                #    ffmpeg step that is already running, and silence there
                #    reads as a hang. When the child hook did not apply, that
                #    wait covers a ranged download in full, so say that instead
                #    of implying the Cancel button reached it.
                if job.is_cancelled() and not cancel_logged:
                    cancel_logged = True
                    note = ('' if FFMPEG_DOWNLOAD_CANCELLABLE else
                            ' A trimmed download is fetched by ffmpeg, which this'
                            ' build cannot interrupt — it will have to finish first.')
                    yield f"data: {json.dumps({'log': '> [FinFetcher] Cancelling — stopping the download...' + note})}\n\n"

                # 3. Check if thread finished
                if not t.is_alive() and msg_queue.empty():
                    break

                # 4. Check for specific result updates (filename, success)
                try:
                    while True:
                        res = result_queue.get_nowait()
                        if 'final_file' in res:
                            final_file = res['final_file']
                        if 'success' in res:
                            download_success = res['success']
                            # A cancel carries no error to show, and leaving
                            # error_payload None also keeps the fast-trim
                            # fallback from re-running the whole download.
                            if not res['success'] and not res.get('cancelled'):
                                error_payload = {'error': res.get('error')}
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
                        if not res['success'] and not res.get('cancelled'):
                            error_payload = {'error': res.get('error')}
            except queue.Empty:
                pass

            return final_file, download_success, error_payload

        def finish_cancelled(final_file, download_success):
            """End the stream after a cancel, leaving nothing half-written.

            What goes and what stays depends on how far the run got. A
            download that reached its final file is a real, complete file the
            user has — cancelling the trim that came after it is no reason to
            destroy it — and a playlist's earlier entries are finished files
            for the same reason. A run stopped mid-flight has only
            part-written output, which yt-dlp has already renamed off ".part"
            by the time the post-processors run, so it would otherwise sit in
            the folder wearing the finished name.
            """
            keep_output = (bool(download_success and final_file)
                           or download_type != 'single')
            for message in _cleanup_download_scraps(save_path, seen_files,
                                                    cancelled=not keep_output,
                                                    since=run_started):
                yield f"data: {json.dumps({'log': message})}\n\n"
            if download_success and final_file:
                yield f"data: {json.dumps({'log': f'> [FinFetcher] The download itself had already finished, so {final_file} was kept.'})}\n\n"
            yield f"data: {json.dumps({'status': 'cancelled'})}\n\n"

        def download_stream():
            trimmed_on_download = False
            final_file, download_success, error_payload = yield from run_attempt(ydl_opts)

            if fast_trim:
                if download_success and final_file:
                    trimmed_on_download = True
                    cut = 'exact cut' if settings['precise_trim'] else 'cut at the nearest keyframes'
                    yield f"data: {json.dumps({'log': f'> [FinFetcher] Fast trim: downloaded only {trim_start} to {trim_end} ({cut}).'})}\n\n"
                elif _is_partial_download_refusal(error_payload):
                    # This source cannot serve a range, so fetch the whole
                    # thing and hand the cut to ffmpeg below. Only for this
                    # specific refusal — retrying a 403 or a dropped
                    # connection would re-download the video for nothing and
                    # blame the wrong thing while doing it.
                    yield f"data: {json.dumps({'log': '> [FinFetcher] Fast trim unavailable for this source — downloading in full and trimming with ffmpeg.'})}\n\n"
                    ydl_opts.pop('download_ranges', None)
                    ydl_opts.pop('force_keyframes_at_cuts', None)
                    final_file, download_success, error_payload = yield from run_attempt(ydl_opts)

            if error_payload:
                yield f"data: {json.dumps(error_payload)}\n\n"

            # Post-download trimming (only when the download could not do it).
            # Never start a fresh re-encode after a cancel — that would be a
            # new ffmpeg process spawned by a run the user has already stopped.
            can_trim = bool(trim_requested and download_success and final_file
                            and not trimmed_on_download and not job.is_cancelled())

            if can_trim and download_type != 'single':
                # A playlist has no single file to trim — the stale checkbox
                # would otherwise re-encode whichever entry finished last.
                yield f"data: {json.dumps({'log': '> [FinFetcher] Trimming is not supported for playlists — skipping.'})}\n\n"
            elif can_trim and not trim_range_ok:
                yield f"data: {json.dumps({'log': f'> [FinFetcher] Invalid trim range ({trim_start} to {trim_end}) — keeping the full download.'})}\n\n"
            elif can_trim:
                try:
                    yield f"data: {json.dumps({'log': f'> [FinFetcher] Trimming video from {trim_start} to {trim_end} with ffmpeg (re-encode)...'})}\n\n"
                    
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
                        # Video trimming - codecs have to suit the container,
                        # which is now user-selectable: webm cannot hold
                        # H.264/AAC, so the old fixed command failed on it.
                        if ext.lower() == '.webm':
                            video_codec = ['-c:v', 'libvpx-vp9', '-crf', '31', '-b:v', '0',
                                           '-c:a', 'libopus', '-b:a', '128k']
                        else:
                            video_codec = ['-c:v', 'libx264', '-preset', 'fast', '-crf', '22',
                                           '-c:a', 'aac', '-b:a', '192k',
                                           '-strict', 'experimental']

                        ffmpeg_cmd = [
                            ffmpeg_exe, '-y',
                            '-i', final_file,
                            '-ss', trim_start,
                            '-to', trim_end,
                        ] + video_codec + [trimmed_file]
                    
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
                    
                    # Cancel has to reach this process directly: it is our
                    # child, not yt-dlp's, and the flag means nothing to it.
                    # An ffmpeg left running past the download is what held
                    # the handle on the file the user could not delete.
                    job.attach_process(trim_proc)
                    try:
                        for tline in trim_proc.stdout:
                            yield f"data: {json.dumps({'log': f'[ffmpeg] {tline.strip()}'})}\n\n"

                        trim_proc.wait()
                    finally:
                        # However this block is left — normally, killed by a
                        # cancel, or because the stream was dropped mid-encode
                        # and the generator is closing — ffmpeg must not
                        # outlive it. An abandoned one keeps its output file
                        # locked, which is the leftover that could not be
                        # deleted.
                        job.detach_process(trim_proc)
                        _terminate_process(trim_proc)

                    # Only replace the original if ffmpeg actually produced something —
                    # an empty output would otherwise destroy the download.
                    trimmed_ok = (
                        trim_proc.returncode == 0
                        and os.path.exists(trimmed_file)
                        and os.path.getsize(trimmed_file) > 0
                    )

                    if job.is_cancelled():
                        # Killed mid-encode, so whatever is on disk is a
                        # fragment of a trim nobody asked to keep. The
                        # untrimmed download stays: it is complete, and the
                        # cancel was of the trim, not of it.
                        removed, error = _remove_file_with_retry(trimmed_file)
                        if error is not None:
                            yield f"data: {json.dumps({'log': f'> [FinFetcher] Trim stopped, but the partial file could not be removed: {error}'})}\n\n"
                        elif removed:
                            yield f"data: {json.dumps({'log': '> [FinFetcher] Trim stopped — the partial trim was removed.'})}\n\n"
                        else:
                            yield f"data: {json.dumps({'log': '> [FinFetcher] Trim stopped before it wrote anything.'})}\n\n"
                    elif trimmed_ok:
                        yield f"data: {json.dumps({'log': '> [FinFetcher] Trim successful! Replacing original file...'})}\n\n"
                        try:
                            if os.path.exists(final_file):
                                os.remove(final_file)
                            os.rename(trimmed_file, final_file)
                            yield f"data: {json.dumps({'log': '> [FinFetcher] Ready!'})}\n\n"
                        except Exception as e:
                            yield f"data: {json.dumps({'log': f'> [FinFetcher] Error replacing file: {e}'})}\n\n"
                    else:
                        # Leave the original download untouched and clean up the scrap
                        try:
                            if os.path.exists(trimmed_file):
                                os.remove(trimmed_file)
                        except Exception:
                            pass
                        yield f"data: {json.dumps({'log': f'> [FinFetcher] Trim failed with code {trim_proc.returncode} — keeping the untrimmed download.'})}\n\n"

                except Exception as e:
                    yield f"data: {json.dumps({'log': f'> [FinFetcher] Trim error: {e}'})}\n\n"
            
            # A cancelled run ends here rather than reporting a result: it has
            # neither completed nor failed, and its own cleanup rules differ.
            if job.is_cancelled():
                yield from finish_cancelled(final_file, download_success)
                return

            # Either way — finished, failed, or fell back — the run can have
            # left a keyframe re-encode or a .part file behind. The download
            # thread is done by now, so nothing still being written is at risk.
            for message in _cleanup_download_scraps(save_path, seen_files,
                                                    since=run_started):
                yield f"data: {json.dumps({'log': message})}\n\n"

            # Send final status
            if download_success:
                yield f"data: {json.dumps({'status': 'completed'})}\n\n"
            else:
                 # Error already sent above
                 pass

        def generate():
            """Stream the download, cleaning up after it however it ends.

            A dropped stream — the window closes, the request is aborted —
            never reaches the cleanup inside download_stream, so it happens
            here instead. A generator cannot yield while it is closing, so
            that path cleans up silently, and only once no attempt is still
            writing: deleting the .part file of a live download would be
            worse than leaving it.
            """
            completed = False
            try:
                for chunk in download_stream():
                    yield chunk
                completed = True
            finally:
                # However this ended, there is no longer a download for
                # /api/download/cancel to act on.
                _clear_current_download_job(job)
                if not completed and not any(t.is_alive() for t in attempt_threads):
                    _cleanup_download_scraps(save_path, seen_files,
                                             cancelled=job.is_cancelled()
                                             and download_type == 'single',
                                             since=run_started)

        _set_current_download_job(job)
        return Flask.response_class(generate(), mimetype='text/event-stream')

    except Exception as e:
        # Nothing is going to run, so nothing should look cancellable
        _clear_current_download_job(job)
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
