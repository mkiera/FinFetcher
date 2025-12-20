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
import yt_dlp
from flask import Flask, request, jsonify, send_from_directory

VERSION = "1.0.0"

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
    try:
        result = subprocess.run(['ffmpeg', '-version'], capture_output=True, text=True,
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
    ydl_opts = {
        'outtmpl': os.path.join(save_path, output_template),
        'updatetime': False,
        'noplaylist': True if download_type == 'single' else False,
    }

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
            
            # Post-download trimming (if requested)
            if download_success and trim_start and trim_end and final_file:
                try:
                    yield f"data: {json.dumps({'log': f'> [Aura] Trimming video from {trim_start} to {trim_end}...'})}\n\n"
                    
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
                    ffmpeg_cmd = [
                        'ffmpeg', '-y',
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
                        yield f"data: {json.dumps({'log': '> [Aura] Trim successful! Replacing original file...'})}\n\n"
                        try:
                            if os.path.exists(final_file):
                                os.remove(final_file)
                            os.rename(trimmed_file, final_file)
                            yield f"data: {json.dumps({'log': '> [Aura] Ready!'})}\n\n"
                        except Exception as e:
                            yield f"data: {json.dumps({'log': f'> [Aura] Error replacing file: {e}'})}\n\n"
                    else:
                        yield f"data: {json.dumps({'log': f'> [Aura] Trim failed with code {trim_proc.returncode}'})}\n\n"
                         
                except Exception as e:
                    yield f"data: {json.dumps({'log': f'> [Aura] Trim error: {e}'})}\n\n"
            
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
    api = Api()
    webview.create_window('FinFetcher', app, js_api=api, width=700, height=1200, resizable=True)
    webview.start()
