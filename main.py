import os
import json
import subprocess
import webview
from flask import Flask, request, jsonify, send_from_directory

# API Class for PyWebView
class Api:
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

def get_video_info(url):
    cmd = [
        'yt-dlp',
        '-J',
        '--flat-playlist',
        url
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        raise Exception(result.stderr)
    return json.loads(result.stdout)

@app.route('/api/info', methods=['POST'])
def get_info():
    data = request.json
    url = data.get('url')
    if not url:
        return jsonify({'error': 'No URL provided'}), 400
    
    try:
        info = get_video_info(url)
        # Check if it's a playlist or a single video
        is_playlist = 'entries' in info or info.get('_type') == 'playlist'
        title = info.get('title', 'Unknown Title')
        
        return jsonify({
            'title': title,
            'duration': info.get('duration', 0),
            'is_playlist': is_playlist,
            'formats': info.get('formats', []), # Return formats
            'entries_count': len(info.get('entries', [])) if is_playlist else 1
        })
    except Exception as e:
        return jsonify({'error': str(e)}), 500

@app.route('/api/download', methods=['POST'])
def download():
    data = request.json
    url = data.get('url')
    mode = data.get('mode', 'video') # video, audio, navidrome
    download_type = data.get('type', 'single') # single, playlist
    save_path = data.get('save_path') # Already handled
    log_to_file = data.get('log_to_file', False) # New param
    quality = data.get('quality', 'max')
    trim_start = data.get('trim_start')
    trim_end = data.get('trim_end')
    
    if not url:
        return jsonify({'error': 'No URL provided'}), 400

    cmd = ['yt-dlp']
    
    # Common options
    cmd.extend(['--no-playlist'] if download_type == 'single' else ['--yes-playlist'])
    cmd.extend(['--no-playlist'] if download_type == 'single' else ['--yes-playlist'])
    
    save_path = data.get('save_path')
    if not save_path:
        home = os.path.expanduser("~")
        save_path = os.path.join(home, "Downloads")
    
    # Ensure path exists
    if not os.path.exists(save_path):
        os.makedirs(save_path)

    output_template = '%(title)s.%(ext)s' if mode == 'video' else '%(artist)s - %(title)s.%(ext)s'
    full_output_path = os.path.join(save_path, output_template)
    cmd.extend(['--output', full_output_path])

    if mode == 'audio':
        cmd.extend([
            '-x',
            '--audio-format', 'mp3',
            '--audio-quality', '0', # Best quality
        ])
    else: # video
        # Use simple, permissive format selection with fallbacks
        # Don't require specific codecs - let yt-dlp pick best available
        format_spec = 'bestvideo+bestaudio/best'
        
        if quality == '1080p':
            format_spec = 'bestvideo[height<=1080]+bestaudio/best[height<=1080]/best'
        elif quality != 'max' and quality.endswith('p'):
            try:
                h = int(quality[:-1])
                format_spec = f'bestvideo[height<={h}]+bestaudio/best[height<={h}]/best'
            except:
                pass
            
        cmd.extend([
            '-f', format_spec,
            '--merge-output-format', 'mp4'
        ])

    # Remove yt-dlp trim args to download full video first
    # This prevents timestamp/keyframe issues associated with trimming during download
    
    # We will still ensure MP4 container for the full download
    cmd.extend(['--merge-output-format', 'mp4'])
    
    cmd.append(url)
    
    # Run download
    try:
        def generate():
            environ = os.environ.copy()
            environ["PYTHONDONTWRITEBYTECODE"] = "1"
            
            process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding='utf-8',
                errors='replace',
                env=environ
            )
            
            final_file = None
            
            while True:
                line = process.stdout.readline()
                if not line and process.poll() is not None:
                    break
                if line:
                    line_content = line.strip()
                    yield f"data: {json.dumps({'log': line_content})}\n\n"
                    
                    # Capture filename
                    if '[Merger] Merging formats into' in line:
                         # Quote stripping and path fix
                         parts = line.split('into')
                         if len(parts) > 1:
                             final_file = parts[1].strip().strip('"')
                    elif '[download] Destination:' in line:
                         parts = line.split('Destination:')
                         if len(parts) > 1:
                             path_part = parts[1].strip()
                             # Only set if not already set by Merger (Merger is definitive for final file)
                             if not final_file:
                                 final_file = path_part
                    elif 'Already downloaded:' in line:
                         parts = line.split('Already downloaded:')
                         if len(parts) > 1:
                             final_file = parts[1].strip()
                    
                    # Log to file
                    if log_to_file:
                        try:
                            log_path = os.path.join(save_path, "download_log.txt")
                            with open(log_path, "a", encoding="utf-8") as f:
                                f.write(line_content + "\n")
                        except:
                            pass

            process.wait()
            
            # Manual Trim Logic
            if process.returncode == 0 and trim_start and trim_end and final_file:
                try:
                    yield f"data: {json.dumps({'log': f'> [Aura] Trimming video from {trim_start} to {trim_end}...'})}\n\n"
                    
                    # Construct output filename
                    base, ext = os.path.splitext(final_file)
                    trimmed_file = f"{base}_trimmed{ext}"
                    
                    # FFmpeg precise trim:
                    # -ss BEFORE -i is faster.
                    # -ss AFTER -i is accurate (decodes everything).
                    # User complained of frozen frames -> Acccuracy is paramount.
                    # We accept slower processing for correct output.
                    ffmpeg_cmd = [
                        'ffmpeg', '-y',
                        '-i', final_file,
                        '-ss', trim_start,
                        '-to', trim_end,
                        '-c:v', 'libx264', '-preset', 'fast', '-crf', '22', # Good quality h264
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
                        env=environ
                    )
                    
                    for tline in trim_proc.stdout:
                        # Optional: filter ffmpeg logs or show them?
                        # Showing them helps debug
                        yield f"data: {json.dumps({'log': f'[ffmpeg] {tline.strip()}'})}\n\n"
                        
                    trim_proc.wait()
                    
                    if trim_proc.returncode == 0:
                        yield f"data: {json.dumps({'log': '> [Aura] Trim successful! Replacing original file...'})}\n\n"
                        try:
                            if os.path.exists(final_file):
                                os.remove(final_file)
                            os.rename(trimmed_file, final_file)
                            yield f"data: {json.dumps({'log': '> [Aura] ready!'})}\n\n"
                        except Exception as e:
                            yield f"data: {json.dumps({'log': f'> [Aura] Error replacing file: {e}'})}\n\n"
                    else:
                         yield f"data: {json.dumps({'log': f'> [Aura] Trim failed with code {trim_proc.returncode}'})}\n\n"
                         
                except Exception as e:
                    yield f"data: {json.dumps({'log': f'> [Aura] Trim error: {e}'})}\n\n"
            
            if process.returncode == 0:
                yield f"data: {json.dumps({'status': 'completed'})}\n\n"
            elif process.returncode != 0 and "429" not in str(process.returncode): # Don't send error again if we caught it
                 # If we terminated, returncode might be non-zero
                 pass
            
            if process.returncode != 0:
                 yield f"data: {json.dumps({'error': 'Download failed/Interrupted'})}\n\n"
                 
        return Flask.response_class(generate(), mimetype='text/event-stream')

    except Exception as e:
        return jsonify({'error': str(e)}), 500

if __name__ == '__main__':
    api = Api()
    # Dynamic window size - taller to fit content without scrollbar
    webview.create_window('Aura Downloader', app, js_api=api, width=700, height=1050, resizable=True)
    webview.start()
