/**
 * FinFetcher 🦭
 * Frontend JavaScript for video/music download application.
 */

// State
let currentMode = 'video';
let currentUrl = '';
let videoDuration = 0;
let cachedVideoInfo = null;

// Initialize - check for ffmpeg first
checkSetup();

async function checkSetup() {
    try {
        const response = await fetch('/api/setup/check');
        const data = await response.json();

        if (data.installed) {
            // FFmpeg is installed, show main app
            showMainApp();
        } else {
            // Show setup screen
            showSetupScreen();
        }
    } catch (e) {
        console.error('Setup check failed:', e);
        // If check fails, try to show main app anyway
        showMainApp();
    }
}

function showSetupScreen() {
    document.getElementById('setupScreen').classList.remove('hidden');
    document.getElementById('mainContainer').classList.add('hidden');
}

function showMainApp() {
    document.getElementById('setupScreen').classList.add('hidden');
    document.getElementById('mainContainer').classList.remove('hidden');

    // Initialize main app
    selectMode(currentMode);
    loadVersion();
}

async function installFFmpeg() {
    const buttons = document.getElementById('setupButtons');
    const progress = document.getElementById('setupProgress');
    const progressFill = document.getElementById('setupProgressFill');
    const status = document.getElementById('setupStatus');
    const note = document.querySelector('.setup-note');

    // Hide buttons, show progress
    buttons.classList.add('hidden');
    progress.classList.remove('hidden');
    if (note) note.classList.add('hidden');

    try {
        const response = await fetch('/api/setup/install-sync', { method: 'POST' });
        const reader = response.body.getReader();
        const decoder = new TextDecoder();

        while (true) {
            const { value, done } = await reader.read();
            if (done) break;

            const chunk = decoder.decode(value);
            const lines = chunk.split('\n\n');

            for (const line of lines) {
                if (line.startsWith('data: ')) {
                    try {
                        const data = JSON.parse(line.substring(6));

                        // Update progress bar
                        progressFill.style.width = data.percent + '%';
                        status.textContent = data.status;

                        // Check for completion
                        if (data.success === true) {
                            setTimeout(() => {
                                showMainApp();
                            }, 1000);
                            return;
                        } else if (data.success === false) {
                            // Installation failed
                            status.textContent = 'Installation failed. Please try again or browse manually.';
                            status.style.color = '#ff6b6b';
                            buttons.classList.remove('hidden');
                            progress.classList.add('hidden');
                            if (note) note.classList.remove('hidden');
                            return;
                        }
                    } catch (e) {
                        // Ignore JSON parse errors
                    }
                }
            }
        }
    } catch (e) {
        console.error('Install error:', e);
        status.textContent = 'Error: ' + e.message;
        status.style.color = '#ff6b6b';
        buttons.classList.remove('hidden');
        progress.classList.add('hidden');
        if (note) note.classList.remove('hidden');
    }
}

async function browseFFmpeg() {
    try {
        const path = await window.pywebview.api.select_folder();
        if (path) {
            const response = await fetch('/api/setup/browse', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ path: path })
            });
            const data = await response.json();

            if (data.success) {
                showMainApp();
            } else {
                alert(data.error || 'FFmpeg not found in selected folder. Please select a folder containing ffmpeg.exe');
            }
        }
    } catch (e) {
        console.error('Browse error:', e);
        alert('Error selecting folder: ' + e.message);
    }
}

async function exitApp() {
    try {
        await fetch('/api/setup/exit', { method: 'POST' });
    } catch (e) {
        // App should be closing
    }
}

// Load version from version.txt
async function loadVersion() {
    try {
        const response = await fetch('/version.txt');
        const version = (await response.text()).trim();
        document.getElementById('versionDisplay').textContent = `v${version} · Made by Kiera`;
    } catch (e) {
        document.getElementById('versionDisplay').textContent = 'v1.0.0 · Made by Kiera';
    }
}

// Fetch video info when URL input loses focus
document.getElementById('urlInput').addEventListener('blur', async (e) => {
    const url = e.target.value.trim();
    if (url && url !== currentUrl) {
        currentUrl = url;
        cachedVideoInfo = await fetchVideoInfo(url);
    }
});

function selectMode(mode) {
    currentMode = mode;
    document.querySelectorAll('.option-card').forEach(card => {
        card.classList.remove('selected');
        if (card.dataset.mode === mode) {
            card.classList.add('selected');
        }
    });

    // Hide advanced options for stream mode (no download settings needed)
    const advancedOptions = document.querySelector('.advanced-options');
    const downloadBtn = document.getElementById('downloadBtn');

    if (mode === 'stream') {
        advancedOptions.style.display = 'none';
        downloadBtn.textContent = 'Stream';
    } else {
        advancedOptions.style.display = '';
        downloadBtn.textContent = 'Download';
    }
}

function toggleAdvanced() {
    document.getElementById('advancedContent').classList.toggle('hidden');
    const chevron = document.querySelector('.advanced-header .chevron');
    chevron.textContent = chevron.textContent === '▼' ? '▲' : '▼';
}

function toggleTrimInputs() {
    const isChecked = document.getElementById('trimToggle').checked;
    const inputs = document.getElementById('trimInputs');
    inputs.classList.toggle('hidden', !isChecked);
}

function log(message) {
    const container = document.getElementById('logContainer');
    const entry = document.createElement('div');
    entry.className = 'log-entry';
    entry.textContent = `> ${message}`;
    container.appendChild(entry);
    container.scrollTop = container.scrollHeight;
}

async function fetchVideoInfo(url, preserveState = false) {
    try {
        document.getElementById('urlStatus').textContent = "🦭 Your seal is fetching...";
        document.querySelector('#logContainer').innerHTML = "<div class='log-entry'>> Your seal is diving for metadata...</div>";
        document.getElementById('previewPanel').classList.add('hidden');

        const response = await fetch('/api/info', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ url: url })
        });
        const data = await response.json();

        if (data.error) throw new Error(data.error);

        videoDuration = data.duration || 0;
        if (!preserveState) {
            initializeSlider();
        }

        // Populate quality dropdown from available formats
        const qualitySelect = document.getElementById('qualitySelect');
        const previousValue = preserveState ? qualitySelect.value : null;

        qualitySelect.innerHTML = '<option value="max">Max (4K/8K)</option><option value="1080p">1080p Compatible</option>';
        if (data.formats && data.formats.length > 0) {
            const heights = [...new Set(data.formats.map(f => f.height).filter(h => h && h >= 144))].sort((a, b) => b - a);
            if (heights.length > 0) {
                qualitySelect.innerHTML = '<option value="max">Max (Best)</option>';
                heights.forEach(h => {
                    const opt = document.createElement('option');
                    opt.value = h + 'p';
                    opt.textContent = `${h}p`;
                    qualitySelect.appendChild(opt);
                });
            }
        }

        // Restore previous selection if still available
        if (previousValue && Array.from(qualitySelect.options).some(o => o.value === previousValue)) {
            qualitySelect.value = previousValue;
        }

        // Populate preview panel
        populatePreviewPanel(data);

        document.getElementById('urlStatus').textContent = "✅ Ready to download";
        document.querySelector('#logContainer').innerHTML = "<div class='log-entry'>> Ready to download! ✅</div>";
        return data;
    } catch (e) {
        console.error(e);
        recordError(e.message);
        document.getElementById('urlStatus').textContent = "❌ Your seal couldn't find that";
        document.querySelector('#logContainer').innerHTML += `<div class='log-entry'>> Your seal couldn't find that: ${e.message}</div>`;
        document.getElementById('previewPanel').classList.add('hidden');
        return null;
    }
}

function populatePreviewPanel(data) {
    const panel = document.getElementById('previewPanel');
    const thumb = document.getElementById('previewThumb');
    const title = document.getElementById('previewTitle');
    const singleVideoMeta = document.getElementById('singleVideoMeta');
    const duration = document.getElementById('previewDuration');
    const size = document.getElementById('previewSize');
    const playlistDropdown = document.getElementById('playlistDropdown');
    const singleVideoOptions = document.getElementById('singleVideoOptions');

    // Set thumbnail
    if (data.thumbnail) {
        thumb.src = data.thumbnail;
        thumb.style.display = 'block';
    } else {
        thumb.style.display = 'none';
    }

    // Set title
    title.textContent = data.title || 'Unknown Title';

    // Handle playlist vs single video
    if (data.is_playlist && data.entries) {
        // Playlist mode
        singleVideoMeta.classList.add('hidden');
        playlistDropdown.classList.remove('hidden');
        document.getElementById('playlistCount').textContent = `${data.entries_count} videos in playlist`;

        // Disable Quality and Trim for playlists
        singleVideoOptions.classList.add('disabled-for-playlist');

        // Populate playlist entries
        const entriesContainer = document.getElementById('playlistEntries');
        entriesContainer.innerHTML = '';

        data.entries.forEach((entry, index) => {
            const entryEl = document.createElement('div');
            entryEl.className = 'playlist-entry';
            entryEl.innerHTML = `
                <div class="playlist-entry-header">
                    <span class="playlist-entry-title">${index + 1}. ${entry.title}</span>
                    <span class="playlist-entry-duration">${formatTime(entry.duration || 0)}</span>
                </div>
            `;
            entriesContainer.appendChild(entryEl);
        });
    } else {
        // Single video mode
        singleVideoMeta.classList.remove('hidden');
        playlistDropdown.classList.add('hidden');

        // Enable Quality and Trim for single videos
        singleVideoOptions.classList.remove('disabled-for-playlist');

        // Set duration and size
        duration.textContent = formatTime(data.duration || 0);
        size.textContent = data.size_formatted || '~Unknown';
    }

    // Show panel
    panel.classList.remove('hidden');
}

async function initiateDownload() {
    const urlInput = document.getElementById('urlInput');
    const url = urlInput.value.trim();

    if (!url) {
        alert("Please enter a YouTube URL");
        return;
    }

    // Handle stream mode separately
    if (currentMode === 'stream') {
        startStream(url);
        return;
    }

    document.getElementById('downloadBtn').disabled = true;
    document.getElementById('downloadBtn').textContent = "Starting...";

    // Use cached info if URL hasn't changed
    let data = cachedVideoInfo;
    if (url !== currentUrl || !cachedVideoInfo) {
        currentUrl = url;
        data = await fetchVideoInfo(url, true);
        cachedVideoInfo = data;
    }

    if (data) {
        if (data.is_playlist) {
            document.getElementById('playlistModal').classList.remove('hidden');
        } else {
            startDownload('single');
        }
    } else {
        resetUI();
    }
}

function confirmDownload(type) {
    document.getElementById('playlistModal').classList.add('hidden');
    startDownload(type);
}

// --- Trim Slider Functions ---

function initializeSlider() {
    const rangeStart = document.getElementById('rangeStart');
    const rangeEnd = document.getElementById('rangeEnd');

    rangeStart.max = videoDuration;
    rangeEnd.max = videoDuration;
    rangeStart.value = 0;
    rangeEnd.value = videoDuration;

    updateSlider();
}

function updateSlider(handle) {
    const rangeStart = document.getElementById('rangeStart');
    const rangeEnd = document.getElementById('rangeEnd');
    const startVal = parseInt(rangeStart.value);
    const endVal = parseInt(rangeEnd.value);

    // Prevent handles from crossing
    if (endVal < startVal) {
        if (handle === 'start') {
            rangeStart.value = endVal;
        } else {
            rangeEnd.value = startVal;
        }
    }

    const min = parseInt(rangeStart.min);
    const max = parseInt(rangeStart.max);
    const currentStart = parseInt(rangeStart.value);
    const currentEnd = parseInt(rangeEnd.value);

    // Update visual fill
    const fill = document.getElementById('sliderFill');
    const percentStart = ((currentStart - min) / (max - min)) * 100;
    const percentEnd = ((currentEnd - min) / (max - min)) * 100;
    fill.style.left = percentStart + "%";
    fill.style.width = (percentEnd - percentStart) + "%";

    // Sync text inputs
    document.getElementById('trimStart').value = formatTime(currentStart);
    document.getElementById('trimEnd').value = formatTime(currentEnd);
}

function updateFromText() {
    const startText = document.getElementById('trimStart').value;
    const endText = document.getElementById('trimEnd').value;

    const startSec = parseTime(startText);
    const endSec = parseTime(endText);

    if (startSec !== null) document.getElementById('rangeStart').value = startSec;
    if (endSec !== null) document.getElementById('rangeEnd').value = endSec;

    updateSlider(null);
}

function formatTime(seconds) {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);

    if (h > 0) {
        return `${h}:${pad(m)}:${pad(s)}`;
    }
    return `${pad(m)}:${pad(s)}`;
}

function pad(num) {
    return num.toString().padStart(2, '0');
}

function parseTime(timeStr) {
    if (!timeStr) return 0;
    const parts = timeStr.split(':').map(Number);
    if (parts.some(isNaN)) return null;

    let seconds = 0;
    if (parts.length === 3) {
        seconds = parts[0] * 3600 + parts[1] * 60 + parts[2];
    } else if (parts.length === 2) {
        seconds = parts[0] * 60 + parts[1];
    } else if (parts.length === 1) {
        seconds = parts[0];
    }
    return seconds;
}

// --- Download Functions ---

async function startDownload(type) {
    const progressArea = document.getElementById('progressArea');
    progressArea.classList.remove('hidden');
    document.getElementById('downloadBtn').textContent = "Downloading...";

    log(`Starting ${type} download...`);
    log(`Mode: ${currentMode}`);

    // Collect options before folder dialog (pywebview can cause UI state issues)
    const logToFile = document.getElementById('logToggle').checked;
    const quality = document.getElementById('qualitySelect').value;
    const trim = document.getElementById('trimToggle').checked;
    let trimStart = null;
    let trimEnd = null;

    if (trim) {
        trimStart = document.getElementById('trimStart').value.trim();
        trimEnd = document.getElementById('trimEnd').value.trim();
        if (!trimStart || !trimEnd) {
            alert("Please enter start and end times for trimming (e.g. 00:30, 01:45)");
            resetUI();
            return;
        }
    }

    let savePath = null;
    const locationToggle = document.getElementById('locationToggle');

    if (locationToggle && locationToggle.checked) {
        log("Select download folder...");
        try {
            savePath = await window.pywebview.api.select_folder();
            if (!savePath) {
                log("Download cancelled (no folder selected).");
                resetUI();
                return;
            }
            log(`Saving to: ${savePath}`);
        } catch (e) {
            log("Error selecting folder: " + e);
        }
    }

    // Start download request
    fetch('/api/download', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            url: currentUrl,
            mode: currentMode,
            type: type,
            save_path: savePath,
            log_to_file: logToFile,
            quality: quality,
            trim_start: trimStart,
            trim_end: trimEnd
        })
    }).then(async response => {
        const reader = response.body.getReader();
        const decoder = new TextDecoder();

        while (true) {
            const { value, done } = await reader.read();
            if (done) break;

            const chunk = decoder.decode(value);
            const lines = chunk.split('\n\n');

            for (const line of lines) {
                if (line.startsWith('data: ')) {
                    const jsonStr = line.substring(6);
                    try {
                        const msg = JSON.parse(jsonStr);
                        if (msg.log) {
                            log(msg.log);
                        }
                        if (msg.status === 'completed') {
                            log("Download Complete! ✅");
                            resetUI();
                        }
                        if (msg.error) {
                            log("Error: " + msg.error);
                            resetUI();
                        }
                    } catch (e) {
                        // Ignore partial JSON
                    }
                }
            }
        }
    }).catch(e => {
        log("Network Error: " + e.message);
        resetUI();
    });
}

function resetUI() {
    document.getElementById('downloadBtn').disabled = false;
    document.getElementById('downloadBtn').textContent = "Download";
    toggleTrimInputs();

    // Ensure Advanced Options visibility matches chevron state
    const advancedContent = document.getElementById('advancedContent');
    const chevron = document.querySelector('.advanced-header .chevron');
    if (chevron.textContent === '▲') {
        advancedContent.classList.remove('hidden');
    }

    // Restore button text based on mode
    document.getElementById('downloadBtn').textContent = currentMode === 'stream' ? 'Stream' : 'Download';
}

// --- Streaming Functions ---

async function startStream(url) {
    const streamModal = document.getElementById('streamModal');
    const streamTitle = document.getElementById('streamTitle');
    const streamPlayer = document.getElementById('streamPlayer');
    const streamStatus = document.getElementById('streamStatus');
    const downloadBtn = document.getElementById('downloadBtn');

    // Show modal immediately with loading state
    streamModal.classList.remove('hidden');
    streamTitle.textContent = 'Loading...';
    streamStatus.textContent = '🦭 Your seal is fetching the stream...';
    streamStatus.className = 'stream-status loading';
    streamPlayer.src = '';

    downloadBtn.disabled = true;
    downloadBtn.textContent = 'Streaming...';

    try {
        const response = await fetch('/api/stream', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ url: url })
        });

        const data = await response.json();

        if (data.error) {
            throw new Error(data.error);
        }

        // Set video source and title
        streamTitle.textContent = data.title || 'Video';
        streamPlayer.src = data.stream_url;
        streamStatus.textContent = '';
        streamStatus.className = 'stream-status';

        // Handle video errors
        streamPlayer.onerror = () => {
            streamStatus.textContent = '❌ Playback error. Try a different video or download instead.';
            streamStatus.className = 'stream-status error';
        };

        // Reset button when video starts
        streamPlayer.onplay = () => {
            downloadBtn.disabled = false;
            downloadBtn.textContent = 'Stream';
        };

    } catch (e) {
        console.error('Stream error:', e);
        recordError(e.message);
        streamTitle.textContent = 'Stream Error';
        streamStatus.textContent = `❌ ${e.message}`;
        streamStatus.className = 'stream-status error';
        downloadBtn.disabled = false;
        downloadBtn.textContent = 'Stream';
    }
}

function closeStream() {
    const streamModal = document.getElementById('streamModal');
    const streamPlayer = document.getElementById('streamPlayer');

    // Stop and clear video
    streamPlayer.pause();
    streamPlayer.src = '';

    // Hide modal
    streamModal.classList.add('hidden');

    // Reset button
    document.getElementById('downloadBtn').disabled = false;
    document.getElementById('downloadBtn').textContent = 'Stream';
}

// --- Debug Panel ---

let lastError = null;

// Keyboard shortcut: Ctrl+Shift+D
document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.shiftKey && e.key === 'D') {
        e.preventDefault();
        toggleDebug();
    }
});

function toggleDebug() {
    const panel = document.getElementById('debugPanel');
    const isHidden = panel.classList.contains('hidden');

    if (isHidden) {
        panel.classList.remove('hidden');
        loadDebugInfo();
    } else {
        panel.classList.add('hidden');
    }
}

async function loadDebugInfo() {
    try {
        const response = await fetch('/api/debug');
        const data = await response.json();

        // System Info
        const sysInfo = data.system;
        document.getElementById('debugSystemInfo').textContent =
            `OS: ${sysInfo.os} ${sysInfo.os_version}\n` +
            `Platform: ${sysInfo.platform}\n` +
            `Python: ${sysInfo.python_version.split(' ')[0]}\n` +
            `Python Path: ${sysInfo.python_executable}`;

        // Dependencies
        const deps = data.dependencies;
        document.getElementById('debugDependencies').textContent =
            `yt-dlp: ${deps['yt-dlp']}\n` +
            `ffmpeg: ${deps['ffmpeg']}`;

        // Last error
        if (lastError) {
            document.getElementById('debugLastError').textContent = lastError;
        }

    } catch (e) {
        document.getElementById('debugSystemInfo').textContent = `Error loading debug info: ${e.message}`;
    }
}

async function runDiagnostic() {
    const resultEl = document.getElementById('debugTestResult');
    resultEl.textContent = '🔄 Running diagnostic test...';

    try {
        const response = await fetch('/api/debug/test', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({})
        });
        const data = await response.json();

        if (data.success) {
            resultEl.textContent = `✅ SUCCESS!\n${data.message}\nVideo: "${data.title}"`;
        } else {
            resultEl.textContent = `❌ FAILED: ${data.message}\n\nError:\n${data.error}`;
        }
    } catch (e) {
        resultEl.textContent = `❌ Request failed: ${e.message}`;
    }
}

function copyDebugInfo() {
    const sysInfo = document.getElementById('debugSystemInfo').textContent;
    const deps = document.getElementById('debugDependencies').textContent;
    const testResult = document.getElementById('debugTestResult').textContent;
    const lastErr = document.getElementById('debugLastError').textContent;

    const fullInfo = `=== FinFetcher Debug Info ===\n\n` +
        `--- System ---\n${sysInfo}\n\n` +
        `--- Dependencies ---\n${deps}\n\n` +
        `--- Test Result ---\n${testResult || 'Not run'}\n\n` +
        `--- Last Error ---\n${lastErr}`;

    navigator.clipboard.writeText(fullInfo).then(() => {
        alert('Debug info copied to clipboard!');
    }).catch(() => {
        // Fallback for older browsers
        console.log(fullInfo);
        alert('Could not copy. Check console for debug info.');
    });
}

// Track errors globally
function recordError(message) {
    lastError = `[${new Date().toLocaleTimeString()}] ${message}`;
}
