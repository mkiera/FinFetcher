import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import {frontend, tick} from './frontend_harness.mjs';

test('every rendered control has a script listener and markup contains no inline handlers', async () => {
    const html = fs.readFileSync(new URL('../index.html', import.meta.url), 'utf8');
    assert.doesNotMatch(html, /\son[a-z]+\s*=/i);
    const app = await frontend();
    let bound = 0;
    for (const control of app.controls) {
        for (const event of ['click', 'change', 'input']) {
            if (control.dataset[event]) {
                assert.equal(typeof control.listeners.get(event), 'function', `${control.dataset[event]} has no ${event} listener`);
                bound++;
            }
        }
    }
    assert.equal(bound, 63);
});

test('clicking Download reaches the native folder dialog and exact download options', async () => {
    const app = await frontend(async (command, payload, emit) => {
        if (command === 'desktop_select_folder') return 'D:\\Chosen clips';
        if (payload.route === '/api/info') return {title: 'Fixture', duration: 10, formats: [{height: 1080}], is_playlist: false};
        if (command === 'desktop_stream') {
            emit({id: payload.streamId, data: {status: 'completed'}});
            emit({id: payload.streamId, done: true});
        }
    });
    app.elements.get('urlInput').value = 'https://example.test/clip';
    app.elements.get('locationToggle').checked = true;
    await app.elements.get('downloadBtn').fire('click');
    await tick();
    assert.equal(app.calls.filter(call => call.command === 'desktop_select_folder').length, 1);
    const download = app.calls.find(call => call.command === 'desktop_stream');
    assert.equal(download.payload.payload.save_path, 'D:\\Chosen clips');
    assert.equal(download.payload.payload.url, 'https://example.test/clip');
    assert.equal(download.payload.payload.pass_to_flipperclipper, false);
    assert.equal(app.elements.get('downloadBtn').disabled, false);
});

test('mode, settings, trim and destination controls work through registered listeners', async () => {
    const app = await frontend();
    const click = (action, key, value) => app.controls.find(element => element.dataset.click === action && (!key || element.dataset[key] === value)).fire('click');
    await click('selectMode', 'mode', 'audio');
    assert.equal(app.run('currentMode'), 'audio');
    await click('toggleSettings');
    assert.equal(app.elements.get('settingsModal').classList.contains('hidden'), false);
    await click('switchSettingsSection', 'section', 'downloads');
    assert.equal(app.elements.get('settingsSectionDownloads').classList.contains('hidden'), false);
    await tick();
    app.elements.get('containerSelect').value = 'mkv';
    await app.elements.get('containerSelect').fire('change');
    assert.equal(app.calls.findLast(call => call.payload.route === '/api/settings' && call.payload.method === 'POST').payload.payload.container, 'mkv');
    app.elements.get('trimToggle').checked = true;
    await app.elements.get('trimToggle').fire('change');
    assert.equal(app.elements.get('trimInputs').classList.contains('hidden'), false);
    app.elements.get('rangeStart').min = '0';
    app.elements.get('rangeStart').max = '10';
    app.elements.get('rangeStart').value = '3';
    app.elements.get('rangeEnd').value = '7';
    await app.elements.get('rangeStart').fire('input');
    assert.equal(app.elements.get('trimStart').value, '00:03');
    app.run("destinationRequest = {id: 'direct-conflict', paths: ['C:/clip.mp4']}");
    await click('chooseDownloadDestination', 'destination', 'overwrite');
    assert.equal(app.calls.find(call => call.payload.route === '/api/download/destination').payload.payload.action, 'overwrite');
});

test('startup loads native services and the actual application version', async () => {
    const app = await frontend();
    assert.equal(app.elements.get('mainContainer').classList.contains('hidden'), false);
    assert.match(app.elements.get('versionDisplay').textContent, /^v2\.0\.0-beta\.1/);
    assert.equal(app.elements.get('flipperClipperBtn').classList.contains('hidden'), false);
    assert.equal(app.calls.some(call => call.payload.route === '/api/update/check'), false);
    const html = fs.readFileSync(new URL('../index.html', import.meta.url), 'utf8');
    assert.ok(html.indexOf('src="desktop.js"') < html.indexOf('src="script.js"'));
    const script = fs.readFileSync(new URL('../script.js', import.meta.url), 'utf8');
    assert.doesNotMatch(script, /pywebview|\bfetch\(/);
});

test('download cancellation stays available before progress and restores idle state after cleanup', async () => {
    let streamId;
    const app = await frontend(async (command, payload, emit) => {
        if (command === 'desktop_stream') streamId = payload.streamId;
        if (payload.route === '/api/download/cancel') {
            emit({id: streamId, data: {status: 'cancelled'}});
            emit({id: streamId, data: {log: 'Temporary files removed'}});
            emit({id: streamId, done: true});
            return {success: true};
        }
    });
    app.run("currentUrl = 'https://example.test/video'");
    const download = app.context.startDownload('single');
    assert.equal(app.elements.get('cancelBtn').classList.contains('hidden'), false);
    await tick();
    await app.context.cancelDownload();
    await download;
    assert.match(app.logs(), /Download cancelled/);
    assert.match(app.logs(), /Temporary files removed/);
    assert.doesNotMatch(app.logs(), /Complete|unexpected/i);
    assert.equal(app.elements.get('downloadBtn').disabled, false);
    assert.equal(app.elements.get('cancelBtn').classList.contains('hidden'), true);
    assert.equal(app.subscriptions.size, 0);
});

test('destination conflicts retain the exact paths and native folder cancellation keeps the choice open', async () => {
    let selectedPath = null;
    const app = await frontend(async (command, payload, emit) => {
        if (command === 'desktop_select_folder') return selectedPath;
        if (command === 'desktop_stream') emit({id: payload.streamId, data: {
            destination_request: {id: 'conflict-1', paths: ['D:\\動画 & Clips\\video.mp4']},
        }});
    });
    const download = app.context.startDownload('single');
    await tick();
    assert.equal(app.elements.get('destinationPaths').textContent, 'D:\\動画 & Clips\\video.mp4');
    assert.equal(app.document.activeElement.id, 'destinationCancel');
    await app.context.chooseDownloadDestination('folder');
    assert.equal(app.elements.get('destinationModal').classList.contains('hidden'), false);
    assert.equal(app.calls.some(call => call.payload.route === '/api/download/destination'), false);
    selectedPath = 'E:\\New folder & clips';
    await app.context.chooseDownloadDestination('folder');
    const choice = app.calls.find(call => call.payload.route === '/api/download/destination');
    assert.deepEqual(JSON.parse(JSON.stringify(choice.payload.payload)), {id: 'conflict-1', action: 'folder', path: selectedPath});
    assert.equal(app.elements.get('destinationModal').classList.contains('hidden'), true);
    const streamId = app.calls.find(call => call.command === 'desktop_stream').payload.streamId;
    app.emit({id: streamId, data: {status: 'completed'}});
    app.emit({id: streamId, done: true});
    await download;
});

test('destination keyboard focus cycles inside the prompt and Escape cancels that exact conflict', async () => {
    const app = await frontend(async (command, payload, emit) => {
        if (command === 'desktop_stream') emit({id: payload.streamId, data: {
            destination_request: {id: 'conflict-keyboard', paths: ['C:\\video.mp4']},
        }});
    });
    const download = app.context.startDownload('single');
    await tick();
    let prevented = 0;
    app.key({key: 'Tab', preventDefault: () => prevented++});
    assert.equal(prevented, 1);
    assert.notEqual(app.document.activeElement.id, 'destinationCancel');
    app.key({key: 'Tab', shiftKey: true, preventDefault: () => prevented++});
    assert.equal(app.document.activeElement.id, 'destinationCancel');
    app.key({key: 'Escape', preventDefault: () => prevented++});
    await tick();
    const request = app.calls.find(call => call.payload.route === '/api/download/destination');
    assert.equal(request.payload.payload.id, 'conflict-keyboard');
    assert.equal(request.payload.payload.action, 'cancel');
    const id = app.calls.find(call => call.command === 'desktop_stream').payload.streamId;
    app.emit({id, data: {status: 'cancelled'}});
    app.emit({id, done: true});
    await download;
});

test('a failed folder picker does not silently download to a different directory', async () => {
    const app = await frontend(async command => {
        if (command === 'desktop_select_folder') throw 'Folder dialog failed';
    });
    app.elements.get('locationToggle').checked = true;
    await app.context.startDownload('single');
    assert.equal(app.calls.some(call => call.command === 'desktop_stream'), false);
    assert.match(app.logs(), /Folder dialog failed/);
    assert.equal(app.elements.get('downloadBtn').disabled, false);
});

test('duplicate clicks while a folder dialog is open do not open a second dialog or download', async () => {
    let closeFolder;
    const app = await frontend(async command => {
        if (command === 'desktop_select_folder') return new Promise(resolve => {closeFolder = resolve;});
    });
    app.elements.get('locationToggle').checked = true;
    const first = app.context.startDownload('single');
    await app.context.startDownload('single');
    assert.equal(app.calls.filter(call => call.command === 'desktop_select_folder').length, 1);
    closeFolder(null);
    await first;
    assert.equal(app.calls.some(call => call.command === 'desktop_stream'), false);
});

test('downloads that lose their final status report failure and unlock the UI', async () => {
    const app = await frontend(async (command, payload, emit) => {
        if (command === 'desktop_stream') {
            emit({id: payload.streamId, data: {log: 'Starting transfer'}});
            emit({id: payload.streamId, done: true});
        }
    });
    await app.context.startDownload('single');
    assert.match(app.logs(), /ended unexpectedly/);
    assert.doesNotMatch(app.logs(), /Download Complete/);
    assert.equal(app.elements.get('downloadBtn').disabled, false);
    assert.equal(app.elements.get('cancelBtn').classList.contains('hidden'), true);
});

test('stream playback errors release the button and closing before extraction finishes cannot restart playback', async () => {
    let finish;
    const app = await frontend(async (_, payload) => {
        if (payload.route === '/api/stream') return new Promise(resolve => {finish = resolve;});
    });
    app.context.selectMode('stream');
    const pending = app.context.startStream('https://example.test/video');
    app.context.closeStream();
    finish({stream_url: 'https://media.example.test/video.mp4', title: 'Too late'});
    await pending;
    assert.equal(app.elements.get('streamPlayer').src, '');
    assert.equal(app.elements.get('streamModal').classList.contains('hidden'), true);
    const next = app.context.startStream('https://example.test/other');
    finish({stream_url: 'https://media.example.test/other.mp4', title: 'Playing'});
    await next;
    assert.equal(app.elements.get('streamPlayer').src, 'https://media.example.test/other.mp4');
    app.elements.get('streamPlayer').onerror();
    assert.equal(app.elements.get('downloadBtn').disabled, false);
    assert.match(app.elements.get('streamStatus').textContent, /Playback error/);
});

test('settings cannot overwrite stored preferences before they load', async () => {
    const app = await frontend();
    await app.context.saveDownloadSettings();
    assert.equal(app.calls.some(call => call.payload.route === '/api/settings'), false);
    await app.context.loadDownloadSettings();
    app.elements.get('containerSelect').value = 'mkv';
    app.elements.get('audioFormatSelect').value = 'flac';
    app.elements.get('subtitleLangs').value = 'en,es';
    await app.context.saveDownloadSettings();
    const saved = app.calls.find(call => call.payload.route === '/api/settings' && call.payload.method === 'POST');
    assert.equal(saved.payload.payload.container, 'mkv');
    assert.equal(saved.payload.payload.audio_format, 'flac');
    assert.equal(saved.payload.payload.subtitle_langs, 'en,es');
    assert.equal(app.elements.get('audioQualitySelect').disabled, true);
});

test('updates apply only a successfully downloaded native installer path', async () => {
    let succeed = false;
    const app = await frontend(async (command, payload, emit) => {
        if (command === 'desktop_stream') {
            emit({id: payload.streamId, data: succeed
                ? {percent: 100, status: 'Ready', success: true, path: 'C:\\Updates\\FinFetcher-Setup.exe'}
                : {percent: 0, status: 'Checksum mismatch', success: false}});
            emit({id: payload.streamId, done: true});
        }
    });
    app.run("pendingUpdate = {exe_asset: {name: 'FinFetcher-Setup.exe', url: 'https://github.com/mkiera/FinFetcher/releases/download/v2.0.0/FinFetcher-Setup.exe', is_installer: true}}");
    await app.context.startUpdate();
    assert.equal(app.calls.some(call => call.payload.route === '/api/update/apply'), false);
    assert.match(app.elements.get('updateStatus').textContent, /Checksum mismatch/);
    succeed = true;
    await app.context.startUpdate();
    const applied = app.calls.find(call => call.payload.route === '/api/update/apply');
    assert.equal(applied.payload.payload.path, 'C:\\Updates\\FinFetcher-Setup.exe');
    assert.match(app.elements.get('updateStatus').textContent, /will close/);
    assert.equal(app.subscriptions.size, 0);
});

test('debug information reports the Rust executable and release links open externally', async () => {
    const app = await frontend(async (_, payload) => {
        if (payload.route === '/api/debug') return {
            system: {os: 'Windows', os_version: '11', platform: 'x86_64', runtime: 'Rust / Tauri', runtime_version: '2', executable: 'C:\\FinFetcher.exe'},
            dependencies: {'yt-dlp': '2026.09.01', ffmpeg: '8'},
        };
    });
    await app.context.loadDebugInfo();
    assert.match(app.elements.get('debugSystemInfo').textContent, /Rust \/ Tauri/);
    assert.doesNotMatch(app.elements.get('debugSystemInfo').textContent, /Python/);
    const link = app.elements.get('updateViewLink');
    link.href = 'https://github.com/mkiera/FinFetcher/releases';
    let prevented = false;
    await link.listeners.get('click')({currentTarget: link, preventDefault: () => {prevented = true;}});
    assert.equal(prevented, true);
    assert.equal(app.calls.at(-1).command, 'desktop_open_external');
});
