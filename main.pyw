"""
FinFetcher 🦭
A friendly video & music downloader desktop application.
Built with Flask + PyWebView for native desktop experience.
"""

import os
import json
import subprocess
import webview
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
    """Fetch video metadata using yt-dlp."""
    cmd = ['yt-dlp', '-J']
    if flat:
        cmd.append('--flat-playlist')
    cmd.append(url)
    
    # Hide console window on Windows
    startupinfo = None
    creationflags = 0
    if os.name == 'nt':
        startupinfo = subprocess.STARTUPINFO()
        startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        creationflags = subprocess.CREATE_NO_WINDOW
    
    result = subprocess.run(cmd, capture_output=True, text=True, startupinfo=startupinfo, creationflags=creationflags)
    if result.returncode != 0:
        raise Exception(result.stderr)
    return json.loads(result.stdout)


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


@app.route('/api/download', methods=['POST'])
def download():
    """API endpoint to initiate download with yt-dlp."""
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

    # Build yt-dlp command
    cmd = ['yt-dlp', '--no-mtime']  # Keep current date/time instead of video upload date
    cmd.extend(['--no-playlist'] if download_type == 'single' else ['--yes-playlist'])
    
    # Set save path (default to Downloads folder)
    if not save_path:
        save_path = os.path.join(os.path.expanduser("~"), "Downloads")
    if not os.path.exists(save_path):
        os.makedirs(save_path)

    # Output template
    output_template = '%(title)s.%(ext)s' if mode == 'video' else '%(artist)s - %(title)s.%(ext)s'
    cmd.extend(['--output', os.path.join(save_path, output_template)])

    # Mode-specific options
    if mode == 'audio':
        cmd.extend(['-x', '--audio-format', 'mp3', '--audio-quality', '0'])
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
        cmd.extend(['-f', format_spec, '--merge-output-format', 'mp4'])

    cmd.extend(['--merge-output-format', 'mp4'])
    cmd.append(url)
    
    # Run download with streaming output
    try:
        def generate():
            environ = os.environ.copy()
            environ["PYTHONDONTWRITEBYTECODE"] = "1"
            
            # Hide console window on Windows
            startupinfo = None
            creationflags = 0
            if os.name == 'nt':
                startupinfo = subprocess.STARTUPINFO()
                startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
                creationflags = subprocess.CREATE_NO_WINDOW
            
            process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding='utf-8',
                errors='replace',
                env=environ,
                startupinfo=startupinfo,
                creationflags=creationflags
            )
            
            final_file = None
            
            # Stream output to frontend
            while True:
                line = process.stdout.readline()
                if not line and process.poll() is not None:
                    break
                if line:
                    line_content = line.strip()
                    yield f"data: {json.dumps({'log': line_content})}\n\n"
                    
                    # Capture final filename from yt-dlp output
                    if '[Merger] Merging formats into' in line:
                        parts = line.split('into')
                        if len(parts) > 1:
                            final_file = parts[1].strip().strip('"')
                    elif '[download] Destination:' in line:
                        parts = line.split('Destination:')
                        if len(parts) > 1 and not final_file:
                            final_file = parts[1].strip()
                    elif 'Already downloaded:' in line:
                        parts = line.split('Already downloaded:')
                        if len(parts) > 1:
                            final_file = parts[1].strip()
                    
                    # Optional: log to file
                    if log_to_file:
                        try:
                            log_path = os.path.join(save_path, "download_log.txt")
                            with open(log_path, "a", encoding="utf-8") as f:
                                f.write(line_content + "\n")
                        except:
                            pass

            process.wait()
            
            # Post-download trimming (if requested)
            if process.returncode == 0 and trim_start and trim_end and final_file:
                try:
                    yield f"data: {json.dumps({'log': f'> [Aura] Trimming video from {trim_start} to {trim_end}...'})}\n\n"
                    
                    base, ext = os.path.splitext(final_file)
                    trimmed_file = f"{base}_trimmed{ext}"
                    
                    # FFmpeg precise trim with re-encoding for accuracy
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
            if process.returncode == 0:
                yield f"data: {json.dumps({'status': 'completed'})}\n\n"
            else:
                yield f"data: {json.dumps({'error': 'Download failed'})}\n\n"
                 
        return Flask.response_class(generate(), mimetype='text/event-stream')

    except Exception as e:
        return jsonify({'error': str(e)}), 500


if __name__ == '__main__':
    api = Api()
    webview.create_window('FinFetcher', app, js_api=api, width=700, height=1050, resizable=True)
    webview.start()
