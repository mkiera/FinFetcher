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
    sponsorblock = data.get('sponsorblock', False)
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
        format_spec = 'bestvideo+bestaudio/best'
        if quality == '1080p':
            format_spec = 'bestvideo[height<=1080]+bestaudio/best[height<=1080]'
            
        cmd.extend([
            '-f', format_spec,
            '--merge-output-format', 'mp4'
        ])
    
    
    if sponsorblock:
        cmd.extend(['--sponsorblock-remove', 'all'])

    if trim_start and trim_end:
        cmd.extend(['--download-sections', f"*{trim_start}-{trim_end}"])

    cmd.append(url)
    
    # Run download
    try:
        def generate():
            # Add randomized sleep to help avoid 429s slightly, 
            # though usually it's IP based. 
            # We can also handle the error in the output parsing.
            
            process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
                universal_newlines=True
            )
            
            for line in process.stdout:
                line_content = line.strip()
                # Check for 429
                if "HTTP Error 429" in line_content:
                    yield f"data: {json.dumps({'error': 'Error 429: Too Many Requests. YouTube is sort of rate-limiting you. Try again later.'})}\n\n"
                    process.terminate()
                    break
                    
                yield f"data: {json.dumps({'log': line_content})}\n\n"
                
                # Log to file if enabled
                if log_to_file:
                    try:
                        log_path = os.path.join(save_path, "download_log.txt")
                        with open(log_path, "a", encoding="utf-8") as f:
                            f.write(line_content + "\n")
                    except:
                        pass # Don't fail download if logging fails
            
            process.wait()
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
    # Increased window size as requested
    webview.create_window('Aura Downloader', app, js_api=api, width=700, height=900, resizable=True)
    webview.start()
