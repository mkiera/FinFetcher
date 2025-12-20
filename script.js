let currentMode = 'video';
let currentUrl = '';
let videoDuration = 0;

// Select Default Mode
selectMode(currentMode);

// Add listener for URL input to fetch duration
document.getElementById('urlInput').addEventListener('blur', async (e) => {
    const url = e.target.value.trim();
    if (url && url !== currentUrl) {
        currentUrl = url;
        await fetchVideoInfo(url);
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
    if (isChecked) {
        inputs.classList.remove('hidden');
    } else {
        inputs.classList.add('hidden');
    }
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
        document.getElementById('urlStatus').textContent = "⌛";
        document.querySelector('#logContainer').innerHTML = "<div class='log-entry'>> Retrieving video metadata...</div>";

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

        // Populate Quality Options
        const qualitySelect = document.getElementById('qualitySelect');
        const previousValue = preserveState ? qualitySelect.value : null;

        qualitySelect.innerHTML = '<option value="max">Max (4K/8K)</option><option value="1080p">1080p Compatible</option>';
        if (data.formats && data.formats.length > 0) {
            // Get unique heights >= 144p
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

        // Restore selection if it exists in new options
        if (previousValue && Array.from(qualitySelect.options).some(o => o.value === previousValue)) {
            qualitySelect.value = previousValue;
        }

        document.getElementById('urlStatus').textContent = "✅";
        document.querySelector('#logContainer').innerHTML = "<div class='log-entry'>> Ready to download! ✅</div>";
        return data;
    } catch (e) {
        console.error(e);
        document.getElementById('urlStatus').textContent = "❌";
        document.querySelector('#logContainer').innerHTML += `<div class='log-entry'>> Error: ${e.message}</div>`;
        return null;
    }
}

async function initiateDownload() {
    const urlInput = document.getElementById('urlInput');
    const url = urlInput.value.trim();

    if (!url) {
        alert("Please enter a YouTube URL");
        return;
    }

    currentUrl = url;
    document.getElementById('downloadBtn').disabled = true;
    document.getElementById('downloadBtn').textContent = "Checking...";

    // Check info (or reuse if we have it, but safest to fetch to be sure of playlist status)
    const data = await fetchVideoInfo(url, true);

    if (data) {
        if (data.is_playlist) {
            document.getElementById('playlistModal').classList.remove('hidden');
        } else {
            startDownload('single');
        }
    } else {
        resetUI(); // Fetch failed
    }
}

function confirmDownload(type) {
    document.getElementById('playlistModal').classList.add('hidden');
    startDownload(type);
}

// --- Trim Slider Logic ---
function initializeSlider() {
    const rangeStart = document.getElementById('rangeStart');
    const rangeEnd = document.getElementById('rangeEnd');

    rangeStart.max = videoDuration;
    rangeEnd.max = videoDuration;

    rangeStart.value = 0;
    rangeEnd.value = videoDuration;

    updateSlider(); // Initial sync
}

function updateSlider(handle) {
    const rangeStart = document.getElementById('rangeStart');
    const rangeEnd = document.getElementById('rangeEnd');
    const startVal = parseInt(rangeStart.value);
    const endVal = parseInt(rangeEnd.value);

    // Prevent crossing
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

    // Update Fill
    const fill = document.getElementById('sliderFill');
    const percentStart = ((currentStart - min) / (max - min)) * 100;
    const percentEnd = ((currentEnd - min) / (max - min)) * 100;

    fill.style.left = percentStart + "%";
    fill.style.width = (percentEnd - percentStart) + "%";

    // Update Text Inputs
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

    updateSlider(null); // Sync visual
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

async function startDownload(type) {
    const progressArea = document.getElementById('progressArea');
    progressArea.classList.remove('hidden');
    document.getElementById('downloadBtn').textContent = "Downloading...";

    log(`Starting ${type} download...`);
    log(`Mode: ${currentMode}`);

    // IMPORTANT: Collect Advanced Options BEFORE folder dialog
    // The pywebview folder dialog can cause UI state to reset
    const sponsors = document.getElementById('sponsorToggle').checked;
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

    const eventSource = new EventSource('/api/download-stream?dummy=avoid-cache');

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

    fetch('/api/download', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            url: currentUrl,
            mode: currentMode,
            type: type,
            save_path: savePath,
            sponsorblock: sponsors,
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
                        // ignore partial json
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

    // Ensure Trim Inputs stay visible if checked
    toggleTrimInputs();

    // Preserve Advanced Options visibility (do not close it)
    // No action needed - just don't touch advancedContent
    // Force Advanced Options to be visible if it was open (or just ensure it's not hidden if user had it open)
    // Since we don't track "was open" persistently across reloads (if reload happened), 
    // we can explicitly check if we want it open. 
    // BUT the text "Advanced Options" toggle logic relies on it.
    // Let's just ensure the class is correct.
    const advancedContent = document.getElementById('advancedContent');
    // If the header arrow indicates open (▲), ensure content is shown
    const chevron = document.querySelector('.advanced-header .chevron');
    if (chevron.textContent === '▲') {
        advancedContent.classList.remove('hidden');
    }
}
