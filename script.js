/**
 * Aura Downloader v1.0.0
 * Frontend JavaScript for YouTube/Music download application.
 */

// State
let currentMode = 'video';
let currentUrl = '';
let videoDuration = 0;
let cachedVideoInfo = null;

// Initialize
selectMode(currentMode);
loadVersion();

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
        document.getElementById('urlStatus').textContent = "⌛ Fetching video info...";
        document.querySelector('#logContainer').innerHTML = "<div class='log-entry'>> Retrieving video metadata...</div>";
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
        document.getElementById('urlStatus').textContent = "❌ Error fetching info";
        document.querySelector('#logContainer').innerHTML += `<div class='log-entry'>> Error: ${e.message}</div>`;
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
}
