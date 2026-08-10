/**
 * FinFetcher 🦭
 * Frontend JavaScript for video/music download application.
 */

// State
let currentMode = 'video';
let currentUrl = '';
let videoDuration = 0;
let cachedVideoInfo = null;
let cachedInfoUrl = null;
let canSelfUpdate = true;
// True only while the download stream is open, which is the only window in
// which /api/download/cancel has anything to act on
let downloadInFlight = false;

// Initialize - check for ffmpeg first
checkSetup();

/**
 * Read a Server-Sent-Event stream, buffering across reads.
 * A network chunk can split an event in half, so events are only parsed once a
 * complete "\n\n" terminated block has arrived — otherwise a status or error
 * event landing on a chunk boundary is silently lost.
 * Return false from onEvent to stop reading early.
 */
async function readEventStream(response, onEvent) {
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    try {
        while (true) {
            const { value, done } = await reader.read();
            buffer += done ? decoder.decode() : decoder.decode(value, { stream: true });

            const events = buffer.split('\n\n');
            // The last piece is an incomplete event — keep it for the next read
            buffer = done ? '' : events.pop();

            for (const event of events) {
                const line = event.trim();
                if (!line.startsWith('data: ')) continue;

                let data;
                try {
                    data = JSON.parse(line.substring(6));
                } catch (e) {
                    continue; // malformed event - skip it, keep reading
                }

                if (onEvent(data) === false) return;
            }

            if (done) return;
        }
    } finally {
        try { reader.cancel(); } catch (e) { /* stream already closed */ }
    }
}

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

async function showMainApp() {
    document.getElementById('setupScreen').classList.add('hidden');
    document.getElementById('mainContainer').classList.remove('hidden');

    // Initialize main app
    selectMode(currentMode);
    loadVersion();

    // Only check for updates if the user hasn't turned that off
    const settings = await loadUpdateSettings();
    if (!settings || settings.auto_check_updates !== false) {
        checkForUpdates();
    }
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

    // Put the setup screen back so the user can retry or browse manually
    const showFailure = (message) => {
        status.textContent = message;
        status.style.color = '#ff6b6b';
        buttons.classList.remove('hidden');
        progress.classList.add('hidden');
        if (note) note.classList.remove('hidden');
    };

    try {
        const response = await fetch('/api/setup/install-sync', { method: 'POST' });

        let settled = false;

        await readEventStream(response, (data) => {
            // Update progress bar
            progressFill.style.width = data.percent + '%';
            status.textContent = data.status;

            // Check for completion
            if (data.success === true) {
                settled = true;
                setTimeout(() => {
                    showMainApp();
                }, 1000);
                return false;
            } else if (data.success === false) {
                settled = true;
                showFailure('Installation failed. Please try again or browse manually.');
                return false;
            }
        });

        // Stream ended without telling us either way — don't strand the user
        if (!settled) {
            showFailure('Installation ended unexpectedly. Please try again or browse manually.');
        }
    } catch (e) {
        console.error('Install error:', e);
        showFailure('Error: ' + e.message);
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
        const info = await fetchVideoInfo(url);
        // Only mark the cache valid for the URL that actually produced it —
        // clicking Download blurs the input, so this fetch is still in flight
        cachedVideoInfo = info;
        cachedInfoUrl = info ? url : null;
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

    // Hide advanced options and download location toggle for stream mode
    const advancedOptions = document.querySelector('.advanced-options');
    const downloadLocationToggle = document.getElementById('downloadLocationToggle');
    const downloadBtn = document.getElementById('downloadBtn');

    if (mode === 'stream') {
        advancedOptions.style.display = 'none';
        if (downloadLocationToggle) downloadLocationToggle.style.display = 'none';
        downloadBtn.textContent = 'Stream';
    } else {
        advancedOptions.style.display = '';
        if (downloadLocationToggle) downloadLocationToggle.style.display = '';
        downloadBtn.textContent = 'Download';
    }

    // Stream mode has no download to stop, and the mode cards stay clickable
    // while one is running, so the control has to follow the mode as well
    updateCancelVisibility();
}

// Cancel is only offered when there is something to cancel. Anything else
// would POST to a backend with no running download and then sit on
// "Cancelling..." waiting for a stream event that never comes.
function updateCancelVisibility() {
    const cancelBtn = document.getElementById('cancelBtn');
    const show = downloadInFlight && currentMode !== 'stream';
    cancelBtn.classList.toggle('hidden', !show);
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
        log(`Your seal couldn't find that: ${e.message}`);
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

        // The greyed-out styling doesn't clear the control, so a trim left over
        // from a previous single video would still be submitted with the playlist
        const playlistTrimToggle = document.getElementById('trimToggle');
        playlistTrimToggle.checked = false;
        toggleTrimInputs();

        // Populate playlist entries
        const entriesContainer = document.getElementById('playlistEntries');
        entriesContainer.innerHTML = '';

        // Built with textContent, never innerHTML — entry titles come from the
        // remote site and would otherwise execute as HTML in this window.
        data.entries.forEach((entry, index) => {
            const entryEl = document.createElement('div');
            entryEl.className = 'playlist-entry';

            const header = document.createElement('div');
            header.className = 'playlist-entry-header';

            const entryTitle = document.createElement('span');
            entryTitle.className = 'playlist-entry-title';
            entryTitle.textContent = `${index + 1}. ${entry.title}`;

            const entryDuration = document.createElement('span');
            entryDuration.className = 'playlist-entry-duration';
            entryDuration.textContent = formatTime(entry.duration || 0);

            header.appendChild(entryTitle);
            header.appendChild(entryDuration);
            entryEl.appendChild(header);
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

    // Use cached info only if it was fetched for this exact URL
    currentUrl = url;
    let data = cachedVideoInfo;
    if (url !== cachedInfoUrl || !cachedVideoInfo) {
        data = await fetchVideoInfo(url, true);
        cachedVideoInfo = data;
        cachedInfoUrl = data ? url : null;
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
    const trimToggle = document.getElementById('trimToggle');

    // No usable duration (playlist, live stream, some direct links) means the
    // slider can only ever produce 00:00 → 00:00, so don't offer trimming
    if (!videoDuration || videoDuration <= 0) {
        // Collapse the range too, so a leftover max from the previous video
        // can't clamp anything the user types into the time boxes
        rangeStart.max = 0;
        rangeEnd.max = 0;
        rangeStart.value = 0;
        rangeEnd.value = 0;
        trimToggle.checked = false;
        trimToggle.disabled = true;
        trimToggle.title = 'Trimming needs a video with a known duration';
        toggleTrimInputs();
        return;
    }

    trimToggle.disabled = false;
    trimToggle.title = '';

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

    // Degenerate range (unknown duration) — writing back would clobber whatever
    // the user typed with 00:00 and the fill percentage would be NaN
    if (!(max > min)) return;

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
    // Trimming applies to a single file only — never send it with a playlist
    const trim = type !== 'playlist' && document.getElementById('trimToggle').checked;
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

        const startSec = parseTime(trimStart);
        const endSec = parseTime(trimEnd);
        if (startSec === null || endSec === null || endSec <= startSec) {
            alert("Please enter a valid trim range — the end time must be after the start time.");
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

    // Start download request.
    // Cancel goes up NOW, not once the response starts arriving: the backend
    // registers the job before it returns the stream, and a streaming response
    // does not resolve fetch() until its first event — which, for a trimmed
    // download, is not until ffmpeg has finished. Waiting for that is what left
    // the window with a "Downloading..." button and no way to stop it.
    downloadInFlight = true;
    updateCancelVisibility();

    try {
        const response = await fetch('/api/download', {
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
        });

        if (!response.ok) {
            // The backend rejected the request outright — that's JSON, not a stream
            const err = await response.json().catch(() => ({}));
            log("Error: " + (err.error || `HTTP ${response.status}`));
        } else {
            await readEventStream(response, (msg) => {
                if (msg.log) {
                    log(msg.log);
                }
                if (msg.status === 'completed') {
                    log("Download Complete! ✅");
                }
                // A stopped download is not a finished one. Keep reading rather
                // than returning false — the backend still reports its cleanup
                // after this, and the stream ends on its own.
                if (msg.status === 'cancelled') {
                    log("Download cancelled.");
                }
                if (msg.error) {
                    log("Error: " + msg.error);
                }
            });
        }
    } catch (e) {
        log("Network Error: " + e.message);
    }

    // Always re-enable the UI once the stream is over, however it ended
    resetUI();
}

async function cancelDownload() {
    const cancelBtn = document.getElementById('cancelBtn');
    // A second click would only fire a second POST, so the button reports the
    // request instead of staying live. It is disabled rather than hidden so the
    // user can still see that the cancel is being dealt with.
    if (cancelBtn.disabled) return;
    cancelBtn.disabled = true;
    cancelBtn.textContent = "Cancelling...";
    log("Cancelling download...");

    // The button is left alone on success: the stream reports the cancellation,
    // and resetUI restores the idle state once it closes.
    const failed = (reason) => {
        log("Couldn't cancel: " + reason);
        cancelBtn.disabled = false;
        cancelBtn.textContent = "Cancel";
    };

    try {
        const response = await fetch('/api/download/cancel', { method: 'POST' });
        const data = await response.json().catch(() => ({}));

        if (!response.ok) {
            failed(data.error || `HTTP ${response.status}`);
        } else if (!data.success) {
            // 200 without success means the backend has no download to stop,
            // so reporting the status code would say nothing useful
            failed(data.error || 'the download was not stopped');
        }
    } catch (e) {
        failed(e.message);
    }
}

function resetUI() {
    document.getElementById('downloadBtn').disabled = false;
    document.getElementById('downloadBtn').textContent = "Download";
    toggleTrimInputs();

    // However the stream ended, there is no longer anything to cancel
    downloadInFlight = false;
    const cancelBtn = document.getElementById('cancelBtn');
    cancelBtn.disabled = false;
    cancelBtn.textContent = "Cancel";
    updateCancelVisibility();

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

// --- Settings Modal & Auto-Update Functions ---

let pendingUpdate = null;
let selectedRelease = null;
let currentChannel = 'stable';

function toggleSettings() {
    const modal = document.getElementById('settingsModal');
    const isHidden = modal.classList.contains('hidden');

    if (isHidden) {
        modal.classList.remove('hidden');
        // Load releases for the current tab
        fetchReleases(currentChannel);
        loadDownloadSettings();
    } else {
        modal.classList.add('hidden');
    }
}

// Top-level Updates/Downloads sections. Kept separate from switchUpdateTab
// because that one writes the channel preference to disk as a side effect.
function switchSettingsSection(section) {
    document.querySelectorAll('.settings-tab[data-section]').forEach(tab => {
        tab.classList.toggle('active', tab.dataset.section === section);
    });

    document.getElementById('settingsSectionUpdates').classList.toggle('hidden', section !== 'updates');
    document.getElementById('settingsSectionDownloads').classList.toggle('hidden', section !== 'downloads');
}

function switchUpdateTab(channel) {
    currentChannel = channel;
    // Match on data-channel, not the bare .settings-tab class — the top-level
    // section tabs share that class and would be deactivated here
    document.querySelectorAll('.settings-tab[data-channel]').forEach(tab => {
        tab.classList.toggle('active', tab.dataset.channel === channel);
    });

    // Save channel preference
    saveUpdateSettings();

    // Fetch releases for this channel
    fetchReleases(channel);
}

// Release rows are built with textContent, never innerHTML — versions, branch
// names and error strings all arrive over the network
function showReleasesMessage(listEl, message) {
    listEl.innerHTML = '';
    const messageEl = document.createElement('div');
    messageEl.className = 'releases-loading';
    messageEl.textContent = message;
    listEl.appendChild(messageEl);
}

// Why a release cannot be installed from in here, or null when it can be.
//
// is_installer comes from the backend's _is_installer_name and is false only
// for the bare-exe builds up to 1.2.4: running one of those would launch the
// old version out of the updates folder and change nothing, so apply_update
// refuses it — after the whole download. The row has to say so first.
//
// Tested against false rather than falsiness on purpose: the alpha artifacts
// endpoint sends an exe_asset with no is_installer key at all (the installer
// is picked out of the zip after downloading), and those stay installable.
function releaseBlockedReason(release) {
    if (!release.exe_asset) return 'No download attached to this release.';
    if (release.exe_asset.is_installer === false) {
        return 'Predates the FinFetcher installer, so it cannot replace an installed copy. Install it from GitHub instead.';
    }
    return null;
}

function buildReleaseRow(label, badges, dateText, note) {
    const row = document.createElement('div');
    row.className = 'release-row';

    const info = document.createElement('div');
    info.className = 'release-info';

    const versionEl = document.createElement('span');
    versionEl.className = 'release-version';
    versionEl.textContent = label;
    info.appendChild(versionEl);

    badges.forEach(badge => {
        const badgeEl = document.createElement('span');
        badgeEl.className = `release-badge ${badge.className}`;
        badgeEl.textContent = badge.text;
        info.appendChild(badgeEl);
    });

    const dateEl = document.createElement('span');
    dateEl.className = 'release-date';
    dateEl.textContent = dateText;

    row.appendChild(info);
    row.appendChild(dateEl);

    // The reason travels with the row it explains — a badge alone would say
    // that this one is different without saying what to do about it
    if (note) {
        const noteEl = document.createElement('div');
        noteEl.className = 'release-note';
        noteEl.textContent = note;
        row.appendChild(noteEl);
    }

    return row;
}

async function fetchReleases(channel) {
    const listEl = document.getElementById('releasesList');
    showReleasesMessage(listEl, 'Loading releases...');
    selectedRelease = null;
    document.getElementById('installSelectedBtn').disabled = true;
    document.getElementById('installSelectedBtn').textContent = 'Update';

    try {
        // Alpha channel uses artifacts endpoint
        if (channel === 'alpha') {
            const response = await fetch('/api/update/artifacts');
            const data = await response.json();

            if (data.error) {
                showReleasesMessage(listEl, `Error: ${data.error}`);
                return;
            }

            if (!data.artifacts || data.artifacts.length === 0) {
                showReleasesMessage(listEl, 'No alpha builds found');
                return;
            }

            const footerVersion = document.getElementById('settingsVersionDisplay');
            if (footerVersion) footerVersion.textContent = `v${data.current_version}`;

            listEl.innerHTML = '';
            let firstSelectable = null;

            data.artifacts.forEach(artifact => {
                const date = artifact.published_at
                    ? new Date(artifact.published_at).toLocaleDateString()
                    : '';

                const badges = [];
                if (artifact.version) {
                    badges.push({ text: `v${artifact.version}`, className: 'current-badge' });
                }
                badges.push({ text: artifact.sha, className: 'pre-badge' });

                const row = buildReleaseRow(artifact.branch, badges, date);
                row.dataset.version = artifact.branch;

                if (artifact.exe_asset) {
                    // Give it a synthetic version for the update flow
                    artifact.version = `${artifact.branch}@${artifact.sha}`;
                    row.style.cursor = 'pointer';
                    row.addEventListener('click', () => selectRelease(artifact, row));
                    if (!firstSelectable) {
                        firstSelectable = { release: artifact, row };
                    }
                }

                listEl.appendChild(row);
            });

            if (firstSelectable) {
                selectRelease(firstSelectable.release, firstSelectable.row);
            }
            return;
        }

        // Stable / Pre-release channels use releases endpoint
        const response = await fetch(`/api/update/releases?channel=${channel}`);
        const data = await response.json();

        if (data.error) {
            showReleasesMessage(listEl, `Error: ${data.error}`);
            return;
        }

        if (!data.releases || data.releases.length === 0) {
            showReleasesMessage(listEl, 'No releases found');
            return;
        }

        // Update footer version
        const footerVersion = document.getElementById('settingsVersionDisplay');
        if (footerVersion) footerVersion.textContent = `v${data.current_version}`;

        listEl.innerHTML = '';
        let firstSelectable = null;

        data.releases.forEach(release => {
            const date = release.published_at
                ? new Date(release.published_at).toLocaleDateString()
                : '';

            // The current version is already unselectable for its own reason,
            // so it does not need one of these on top of the 'current' badge
            const blocked = release.is_current ? null : releaseBlockedReason(release);

            const badges = [];
            if (release.is_current) badges.push({ text: 'current', className: 'current-badge' });
            if (release.prerelease) badges.push({ text: 'pre-release', className: 'pre-badge' });
            if (blocked) badges.push({ text: 'manual install', className: 'blocked-badge' });

            const row = buildReleaseRow(`v${release.version}`, badges, date, blocked);
            if (release.is_current) row.classList.add('current');
            if (blocked) row.classList.add('blocked');
            row.dataset.version = release.version;

            if (!release.is_current && !blocked) {
                row.style.cursor = 'pointer';
                row.addEventListener('click', () => selectRelease(release, row));

                // Default-select the first (most recent) non-current release
                if (!firstSelectable) {
                    firstSelectable = { release, row };
                }
            }

            listEl.appendChild(row);
        });

        // Auto-select the most recent selectable release
        if (firstSelectable) {
            selectRelease(firstSelectable.release, firstSelectable.row);
        }

    } catch (e) {
        showReleasesMessage(listEl, `Failed to load: ${e.message}`);
    }
}

function selectRelease(release, rowEl) {
    // Deselect previous
    document.querySelectorAll('.release-row.selected').forEach(r => r.classList.remove('selected'));

    // Select new
    rowEl.classList.add('selected');
    selectedRelease = release;

    const btn = document.getElementById('installSelectedBtn');
    btn.disabled = false;
    // Show branch name for artifacts, version for releases
    if (release.branch) {
        btn.textContent = `Update to ${release.branch}@${release.sha}`;
    } else {
        btn.textContent = `Update to v${release.version}`;
    }
}

async function installSelectedVersion() {
    if (!selectedRelease || !selectedRelease.exe_asset) {
        alert('No version selected or no executable available.');
        return;
    }

    if (!canSelfUpdate) {
        alert('Self-update is only available in the packaged exe — please download the new version manually.');
        return;
    }

    // Store as pending and use starUpdate flow
    pendingUpdate = selectedRelease;

    // Close settings modal
    document.getElementById('settingsModal').classList.add('hidden');

    // Trigger the download and apply flow
    startUpdate();
}

async function checkForUpdates(force = false) {
    try {
        const params = new URLSearchParams();
        if (force) params.set('force', 'true');

        const response = await fetch(`/api/update/check?${params}`);
        const data = await response.json();

        if (data.skipped) return data; // Cooldown or disabled, no need to check
        if (data.error) {
            console.warn('Update check failed:', data.error);
            // Return the payload so callers can show the real reason
            return data;
        }

        if (data.available && data.update) {
            // Don't show banner for skipped versions (unless forced)
            if (data.was_skipped && !force) return data;

            pendingUpdate = data.update;
            showUpdateBanner(data.current_version, data.update);
        }
        return data;
    } catch (e) {
        console.warn('Update check error:', e);
        return null;
    }
}

async function manualCheckForUpdates() {
    const statusEl = document.getElementById('updateCheckStatus');
    const btn = document.getElementById('checkUpdatesBtn');

    btn.disabled = true;
    btn.textContent = 'Checking...';
    statusEl.textContent = '🔄 Checking GitHub for updates...';
    statusEl.className = 'update-check-status';

    try {
        const data = await checkForUpdates(true);

        if (!data) {
            statusEl.textContent = '❌ Could not reach GitHub';
            statusEl.className = 'update-check-status error';
        } else if (data.error) {
            statusEl.textContent = `❌ ${data.error}`;
            statusEl.className = 'update-check-status error';
        } else if (data.available && data.update) {
            statusEl.textContent = `✅ Update available: v${data.update.version}`;
            statusEl.className = 'update-check-status success';
        } else {
            statusEl.textContent = '✅ You are on the latest version! 🦭';
            statusEl.className = 'update-check-status success';
        }

        // Refresh the release list
        fetchReleases(currentChannel);

    } catch (e) {
        statusEl.textContent = `❌ ${e.message}`;
        statusEl.className = 'update-check-status error';
    }

    btn.disabled = false;
    btn.textContent = 'Check for Updates';
}

function showUpdateBanner(currentVersion, update) {
    const banner = document.getElementById('updateBanner');
    const versionInfo = document.getElementById('updateVersionInfo');
    const viewLink = document.getElementById('updateViewLink');
    const installBtn = document.getElementById('updateInstallBtn');
    const manualNote = document.getElementById('updateManualNote');

    versionInfo.textContent = `v${currentVersion} → v${update.version}${update.prerelease ? ' (pre-release)' : ''}`;
    viewLink.href = update.html_url;

    // The check endpoint offers anything from 1.2.0 up, so the banner can land
    // on a release self-update will refuse just as the list can. Take the
    // button away rather than let it spend the download first — View on GitHub
    // is still there, and is the route that actually works.
    const manualOnly = !!releaseBlockedReason(update);
    installBtn.classList.toggle('hidden', manualOnly);
    manualNote.classList.toggle('hidden', !manualOnly);

    banner.classList.remove('hidden');
}

function dismissUpdate() {
    const banner = document.getElementById('updateBanner');
    banner.classList.add('hidden');

    // Save skipped version
    if (pendingUpdate) {
        fetch('/api/update/settings', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ skipped_version: pendingUpdate.version })
        });
    }
}

async function startUpdate() {
    if (!pendingUpdate || !pendingUpdate.exe_asset) {
        alert('No update available to install.');
        return;
    }

    // Backstop for both callers: the banner hides its button and the release
    // list refuses to select these, but neither guard should be the only one
    // standing between a click and a download that ends in a refusal.
    const blocked = releaseBlockedReason(pendingUpdate);
    if (blocked) {
        alert(blocked);
        return;
    }

    if (!canSelfUpdate) {
        alert('Self-update is only available in the packaged exe — please download the new version manually.');
        return;
    }

    // Hide banner, show progress modal
    document.getElementById('updateBanner').classList.add('hidden');
    const modal = document.getElementById('updateModal');
    const progressFill = document.getElementById('updateProgressFill');
    const status = document.getElementById('updateStatus');
    modal.classList.remove('hidden');

    // A failed attempt leaves the status red and the bar full, so clear both
    // before the first progress event lands or the retry looks pre-broken
    status.style.color = '';
    status.textContent = 'Preparing update...';
    progressFill.style.width = '0%';

    const asset = pendingUpdate.exe_asset;
    const params = new URLSearchParams({ url: asset.url, name: asset.name });

    const showFailure = (message) => {
        status.textContent = message;
        status.style.color = '#ff6b6b';
        setTimeout(() => modal.classList.add('hidden'), 3000);
    };

    try {
        const response = await fetch(`/api/update/download?${params}`);

        // A rejected download (e.g. untrusted host) answers with JSON, not a stream
        if (!response.ok) {
            const err = await response.json().catch(() => ({}));
            showFailure('Update failed: ' + (err.error || `HTTP ${response.status}`));
            return;
        }

        let downloadedPath = null;
        let failed = false;

        await readEventStream(response, (data) => {
            progressFill.style.width = data.percent + '%';
            status.textContent = data.status;

            if (data.success === true && data.path) {
                downloadedPath = data.path;
            } else if (data.success === false) {
                failed = true;
                showFailure('Update failed: ' + data.status);
                return false;
            }
        });

        if (failed) return;

        if (!downloadedPath) {
            showFailure('Update failed: the download ended without a file.');
            return;
        }

        status.textContent = 'Starting the installer... 🦭';
        progressFill.style.width = '100%';

        // The backend launches the installer and then exits, so from here on the
        // window simply disappears and setup runs on its own. Whether the app is
        // relaunched afterwards is the installer's call, so nothing below promises it.
        const applyResponse = await fetch('/api/update/apply', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ path: downloadedPath })
        });
        const applyData = await applyResponse.json();

        if (applyData.success) {
            status.textContent = 'FinFetcher will close while the installer runs. 🦭';

            // The app exits about a second after that reply and takes this page
            // with it, so this timer can only fire when the hand-off quietly
            // failed. Say so rather than leave the modal sitting here forever.
            setTimeout(() => {
                if (!modal.classList.contains('hidden')) {
                    showFailure('The installer did not start — install the update from GitHub instead.');
                }
            }, 30000);
        } else {
            showFailure('Could not start the installer: ' + (applyData.error || 'Unknown error'));
        }
    } catch (e) {
        showFailure('Update error: ' + e.message);
    }
}

async function loadUpdateSettings() {
    try {
        const response = await fetch('/api/update/settings');
        const settings = await response.json();

        const autoToggle = document.getElementById('autoUpdateToggle');
        if (autoToggle) autoToggle.checked = settings.auto_check_updates !== false;

        // Set the active channel tab (data-channel only — see switchUpdateTab)
        currentChannel = settings.update_channel || 'stable';
        document.querySelectorAll('.settings-tab[data-channel]').forEach(tab => {
            tab.classList.toggle('active', tab.dataset.channel === currentChannel);
        });

        // Update footer version
        const footerVersion = document.getElementById('settingsVersionDisplay');
        if (footerVersion && settings.current_version) {
            footerVersion.textContent = `v${settings.current_version}`;
        }

        // Self-update only works for the packaged exe
        canSelfUpdate = settings.can_self_update !== false;

        return settings;
    } catch (e) {
        console.warn('Could not load update settings:', e);
        return null;
    }
}

async function saveUpdateSettings() {
    const autoCheck = document.getElementById('autoUpdateToggle').checked;

    try {
        await fetch('/api/update/settings', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                update_channel: currentChannel,
                auto_check_updates: autoCheck
            })
        });
    } catch (e) {
        console.warn('Could not save update settings:', e);
    }
}

// --- Download Settings ---

// Every control in the Downloads panel starts on its markup default, which is
// only a guess at what is stored. Saving before a load has landed would write
// that guess over all seventeen real settings, so the save path waits.
let downloadSettingsLoaded = false;
// Monotonic id so an out-of-order save reply can't revert a newer change
let downloadSettingsSaveSeq = 0;

// A number input holds a string and can be left empty, so nothing reaches the
// backend without being coerced first. Fallbacks match the /api/settings
// defaults, which keeps a blank field from silently meaning zero.
function clampInt(value, min, max, fallback) {
    const parsed = parseInt(value, 10);
    if (isNaN(parsed)) return fallback;
    return Math.min(max, Math.max(min, parsed));
}

// Assigning a value a select has no option for blanks it instead, and that
// blank would be posted straight back on the next save
function setSelectValue(el, value, fallback) {
    const wanted = String(value);
    const known = Array.from(el.options).some(option => option.value === wanted);
    el.value = known ? wanted : fallback;
}

function readSponsorblockCategories() {
    const boxes = document.querySelectorAll('#sponsorblockCategories input[type="checkbox"]');
    return Array.from(boxes).filter(box => box.checked).map(box => box.value);
}

// Controls that only mean something when their parent option is on stay visible
// but inert, so the panel still shows what turning it on would give you
function setSubgroupEnabled(id, enabled) {
    const group = document.getElementById(id);
    group.classList.toggle('disabled', !enabled);
    // The class only greys it out — disabling the controls is what actually
    // keeps them out of reach of the mouse and the tab key
    group.querySelectorAll('input, select').forEach(control => {
        control.disabled = !enabled;
    });
}

function updateMediaDependencies() {
    setSubgroupEnabled('subtitleOptions', document.getElementById('subtitlesToggle').checked);

    // SponsorBlock reads an empty category list as "every category", so the
    // backend has to treat "on with nothing ticked" as off — leaving a toggle
    // that is visibly on and removes nothing. Say which it is.
    const sponsorblockOn = document.getElementById('sponsorblockToggle').checked;
    setSubgroupEnabled('sponsorblockOptions', sponsorblockOn);
    document.getElementById('sponsorblockWarning').classList.toggle(
        'hidden', !sponsorblockOn || readSponsorblockCategories().length > 0);

    // flac and wav are lossless, so yt-dlp's quality setting has nothing to act on
    const audioFormat = document.getElementById('audioFormatSelect').value;
    const lossless = audioFormat === 'flac' || audioFormat === 'wav';
    document.getElementById('audioQualitySelect').disabled = lossless;
    document.getElementById('audioQualityHint').textContent = lossless
        ? `Not used — ${audioFormat} keeps every bit of the source.`
        : 'Lower quality means a smaller file.';
}

function applyDownloadSettings(settings) {
    if (!settings) return;

    document.getElementById('concurrentFragments').value = clampInt(settings.concurrent_fragments, 1, 16, 4);
    // No upper bound in the contract — the backend is the authority on clamping
    document.getElementById('rateLimitKbps').value = clampInt(settings.rate_limit_kbps, 0, Infinity, 0);
    document.getElementById('downloadArchiveToggle').checked = settings.use_download_archive !== false;
    document.getElementById('preciseTrimToggle').checked = settings.precise_trim === true;
    document.getElementById('logToggle').checked = settings.log_to_file === true;

    setSelectValue(document.getElementById('containerSelect'), settings.container, 'mp4');
    setSelectValue(document.getElementById('audioFormatSelect'), settings.audio_format, 'mp3');
    setSelectValue(document.getElementById('audioQualitySelect'), settings.audio_quality, '0');

    document.getElementById('subtitlesToggle').checked = settings.subtitles_enabled === true;
    if (typeof settings.subtitle_langs === 'string') {
        document.getElementById('subtitleLangs').value = settings.subtitle_langs;
    }
    document.getElementById('subtitlesAutoToggle').checked = settings.subtitles_auto === true;
    document.getElementById('embedSubtitlesToggle').checked = settings.embed_subtitles !== false;

    document.getElementById('sponsorblockToggle').checked = settings.sponsorblock_enabled === true;
    // Only touched when a list actually arrived — a missing key read as "none
    // selected" would clear every box and then save that emptiness back
    if (Array.isArray(settings.sponsorblock_categories)) {
        document.querySelectorAll('#sponsorblockCategories input[type="checkbox"]').forEach(box => {
            box.checked = settings.sponsorblock_categories.includes(box.value);
        });
    }

    document.getElementById('embedThumbnailToggle').checked = settings.embed_thumbnail !== false;
    document.getElementById('embedMetadataToggle').checked = settings.embed_metadata !== false;
    document.getElementById('embedChaptersToggle').checked = settings.embed_chapters !== false;
    document.getElementById('splitChaptersToggle').checked = settings.split_chapters === true;

    updateMediaDependencies();
}

async function loadDownloadSettings() {
    const statusEl = document.getElementById('downloadSettingsStatus');

    // A load that failed must not leave the markup defaults on screen looking
    // like saved preferences — say so instead.
    const showLoadFailure = (message) => {
        if (!statusEl) return;
        statusEl.textContent = message;
        statusEl.className = 'update-check-status error';
    };

    try {
        const response = await fetch('/api/settings');
        if (!response.ok) {
            showLoadFailure(`Couldn't load saved settings (HTTP ${response.status}) — changes won't be saved.`);
            return null;
        }

        const settings = await response.json();
        applyDownloadSettings(settings);
        if (settings) downloadSettingsLoaded = true;
        if (statusEl) statusEl.className = 'update-check-status hidden';
        return settings;
    } catch (e) {
        console.warn('Could not load download settings:', e);
        showLoadFailure(`Couldn't load saved settings: ${e.message} — changes won't be saved.`);
        return null;
    } finally {
        // A failed load applies nothing, but the panel is on screen regardless
        // and its dependent controls still have to match the toggles above them
        updateMediaDependencies();
    }
}

async function saveDownloadSettings() {
    if (!downloadSettingsLoaded) {
        console.warn('Download settings have not loaded yet — not saving.');
        return;
    }

    // Follow the toggle straight away rather than waiting for the round trip,
    // so the dependent controls never look live while they are being saved off
    updateMediaDependencies();

    const langs = document.getElementById('subtitleLangs').value.trim();

    // Only the keys this page renders — the whole contract as of now, but the
    // body stays partial on purpose so a key added to the backend later isn't
    // overwritten with a stale value from a control that doesn't exist yet
    const payload = {
        concurrent_fragments: clampInt(document.getElementById('concurrentFragments').value, 1, 16, 4),
        rate_limit_kbps: clampInt(document.getElementById('rateLimitKbps').value, 0, Infinity, 0),
        use_download_archive: document.getElementById('downloadArchiveToggle').checked,
        precise_trim: document.getElementById('preciseTrimToggle').checked,
        log_to_file: document.getElementById('logToggle').checked,
        container: document.getElementById('containerSelect').value,
        audio_format: document.getElementById('audioFormatSelect').value,
        audio_quality: document.getElementById('audioQualitySelect').value,
        subtitles_enabled: document.getElementById('subtitlesToggle').checked,
        // An empty box would ask for no languages at all, which is never what
        // clearing the field is meant to mean
        subtitle_langs: langs || 'en',
        subtitles_auto: document.getElementById('subtitlesAutoToggle').checked,
        embed_subtitles: document.getElementById('embedSubtitlesToggle').checked,
        sponsorblock_enabled: document.getElementById('sponsorblockToggle').checked,
        sponsorblock_categories: readSponsorblockCategories(),
        embed_thumbnail: document.getElementById('embedThumbnailToggle').checked,
        embed_metadata: document.getElementById('embedMetadataToggle').checked,
        embed_chapters: document.getElementById('embedChaptersToggle').checked,
        split_chapters: document.getElementById('splitChaptersToggle').checked
    };

    // Flipping two toggles quickly starts two saves; the replies can land out
    // of order, and each one re-applies a full settings object. Without this,
    // a stale reply would visibly revert the newer change.
    const seq = ++downloadSettingsSaveSeq;

    const statusEl = document.getElementById('downloadSettingsStatus');
    try {
        const response = await fetch('/api/settings', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
        });
        const data = await response.json();

        if (seq !== downloadSettingsSaveSeq) return; // a newer save won

        if (!response.ok || !data || !data.success) {
            const reason = (data && data.error) || `HTTP ${response.status}`;
            if (statusEl) {
                statusEl.textContent = `Couldn't save settings: ${reason}`;
                statusEl.className = 'update-check-status error';
            }
            return;
        }

        // Show what was actually stored, so backend clamping is visible instead
        // of the field keeping a value that was never saved
        if (data.settings) {
            applyDownloadSettings(data.settings);
        }
        if (statusEl) statusEl.className = 'update-check-status hidden';
    } catch (e) {
        console.warn('Could not save download settings:', e);
        if (seq === downloadSettingsSaveSeq && statusEl) {
            statusEl.textContent = `Couldn't save settings: ${e.message}`;
            statusEl.className = 'update-check-status error';
        }
    }
}

// --- Modal dismissal ---

// Escape and a backdrop click close any dismissible modal. Without these the
// only way out of the settings panel is its ✕, which scrolls off the top on a
// long section. The update modal is deliberately excluded: the app is exiting
// to hand over to the installer, so there is nothing to go back to.
const DISMISSIBLE_MODALS = ['playlistModal', 'streamModal', 'debugPanel', 'settingsModal'];

function dismissModal(id) {
    switch (id) {
        case 'streamModal':
            closeStream();
            return true;
        case 'debugPanel':
            toggleDebug();
            return true;
        case 'settingsModal':
            toggleSettings();
            return true;
        case 'playlistModal':
            // Backing out of the prompt has to release the Download button,
            // which initiateDownload disabled before opening it
            document.getElementById('playlistModal').classList.add('hidden');
            resetUI();
            return true;
    }
    return false;
}

document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    // Leaving fullscreen video already consumes Escape; closing the player in
    // the same keypress would be one dismissal too many
    if (document.fullscreenElement) return;

    const open = DISMISSIBLE_MODALS.find(id => {
        const el = document.getElementById(id);
        return el && !el.classList.contains('hidden');
    });
    if (open) dismissModal(open);
});

DISMISSIBLE_MODALS.forEach(id => {
    const el = document.getElementById(id);
    if (!el) return;
    el.addEventListener('click', (e) => {
        // Only the backdrop itself — a click inside the panel must not close it
        if (e.target === el) dismissModal(id);
    });
});

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
            `ffmpeg: ${deps['ffmpeg']}\n` +
            `CA store: ${deps['CA store'] || 'unknown'}`;

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
