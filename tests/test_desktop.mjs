import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const source = fs.readFileSync(new URL('../desktop.js', import.meta.url), 'utf8');

function bridge({invoke = async () => ({}), listen, native = true} = {}) {
    const calls = [];
    const subscriptions = new Map();
    const lifecycle = new Map();
    let subscriptionId = 0;
    let removed = 0;
    const window = {
        addEventListener: (name, callback) => lifecycle.set(name, callback),
        dispatchEvent: () => {},
    };
    const emit = payload => {
        for (const callback of [...subscriptions.values()]) callback({payload});
    };
    if (native) window.__TAURI__ = {
        core: {invoke: async (command, payload) => {
            calls.push({command, payload});
            return invoke(command, payload, emit);
        }},
        event: {listen: async (name, callback) => {
            assert.equal(name, 'finfetcher-stream');
            if (listen) await listen(callback);
            const id = ++subscriptionId;
            subscriptions.set(id, callback);
            return () => { subscriptions.delete(id); removed++; };
        }},
    };
    const context = vm.createContext({window, URL, URLSearchParams, Response, ReadableStream,
        TextEncoder, DOMException, crypto: globalThis.crypto, Event, console});
    vm.runInContext(source, context);
    return {desktop: window.desktop, calls, emit, subscriptions, lifecycle,
        removed: () => removed};
}

test('requests preserve query strings, JSON values, methods and Windows paths', async () => {
    const harness = bridge({invoke: async () => ({success: true})});
    const result = await harness.desktop.request('/api/download/destination?id=old', {
        method: 'post', body: JSON.stringify({id: 'new', path: 'D:\\動画 & Clips\\save', action: 'folder'}),
    });
    assert.equal(result.ok, true);
    assert.deepEqual(await result.json(), {success: true});
    assert.deepEqual(JSON.parse(JSON.stringify(harness.calls)), [{command: 'desktop_request', payload: {
        route: '/api/download/destination', method: 'POST',
        payload: {id: 'new', path: 'D:\\動画 & Clips\\save', action: 'folder'},
    }}]);
    await harness.desktop.request('/api/update/check?force=true');
    assert.equal(harness.calls[1].payload.method, 'GET');
    assert.equal(harness.calls[1].payload.payload.force, 'true');
});

test('application errors are failed responses and native failures retain their message', async () => {
    const harness = bridge({invoke: async (_, {route}) => {
        if (route === '/api/info') return {error: 'That video is unavailable'};
        throw 'Could not save settings';
    }});
    const response = await harness.desktop.request('/api/info');
    assert.equal(response.ok, false);
    assert.equal((await response.json()).error, 'That video is unavailable');
    await assert.rejects(harness.desktop.request('/api/settings'), /Could not save settings/);
});

test('version responses contain the native version and do not invent one', async () => {
    for (const value of ['2.0.0-beta.1', {version: '2.0.0-beta.1'}]) {
        const harness = bridge({invoke: async () => value});
        assert.equal(await (await harness.desktop.request('/version.txt')).text(), '2.0.0-beta.1');
    }
    const harness = bridge({invoke: async () => ({})});
    await assert.rejects(harness.desktop.request('/version.txt'), /version/i);
});

test('folder selection returns exact paths and cancellation stays null', async () => {
    for (const selected of ['D:\\動画 & Clips', null]) {
        const harness = bridge({invoke: async () => selected});
        assert.equal(await harness.desktop.selectFolder(), selected);
        assert.equal(harness.calls[0].command, 'desktop_select_folder');
    }
});

test('missing desktop services and invalid requests fail without network fallback', async () => {
    const harness = bridge({native: false});
    assert.equal(harness.desktop.available, false);
    await assert.rejects(harness.desktop.request('/api/settings'), /desktop/i);
    await assert.rejects(harness.desktop.selectFolder(), /desktop/i);
    const live = bridge();
    for (const url of ['https://other.example/api/settings', '/unexpected', '//other.example/api/settings']) {
        await assert.rejects(live.desktop.request(url), /request/i);
    }
    await assert.rejects(live.desktop.request('/api/settings', {body: '{broken'}));
    assert.equal(live.calls.length, 0);
});

test('streams subscribe before launch and preserve early progress, completion and unicode', async () => {
    const harness = bridge({invoke: async (command, {streamId}, emit) => {
        assert.equal(command, 'desktop_stream');
        assert.equal(harness.subscriptions.size, 1);
        emit({id: streamId, data: {log: '動画 🦭'}});
        emit({id: streamId, data: {status: 'completed'}});
        emit({id: streamId, done: true});
    }});
    const response = await harness.desktop.request('/api/download', {method: 'POST', body: '{"url":"https://example.test/video"}'});
    const text = await response.text();
    assert.equal(text, 'data: {"log":"動画 🦭"}\n\ndata: {"status":"completed"}\n\n');
    assert.equal(harness.calls[0].payload.route, '/api/download');
    assert.equal(harness.calls[0].payload.payload.url, 'https://example.test/video');
    assert.equal(harness.subscriptions.size, 0);
    assert.equal(harness.removed(), 1);
});

test('parallel streams ignore other IDs and release each subscription exactly once', async () => {
    const harness = bridge();
    const setup = await harness.desktop.request('/api/setup/install-sync', {method: 'POST'});
    const update = await harness.desktop.request('/api/update/download?url=https%3A%2F%2Fexample.test%2Fa.exe&name=FinFetcher-Setup.exe');
    const [first, second] = harness.calls.map(call => call.payload.streamId);
    assert.notEqual(first, second);
    harness.emit({id: 'unrelated', data: {error: 'Wrong stream'}});
    harness.emit({id: first, data: {success: true}});
    harness.emit({id: second, data: {path: 'C:\\update.exe', success: true}});
    harness.emit({id: first, done: true});
    harness.emit({id: second, done: true});
    assert.equal(await setup.text(), 'data: {"success":true}\n\n');
    assert.match(await update.text(), /update.exe/);
    assert.equal(harness.removed(), 2);
});

test('duplicate downloads are rejected before a second worker starts', async () => {
    const harness = bridge();
    const response = await harness.desktop.request('/api/download', {method: 'POST'});
    await assert.rejects(harness.desktop.request('/api/download', {method: 'POST'}), /already/i);
    assert.equal(harness.calls.length, 1);
    harness.emit({id: harness.calls[0].payload.streamId, done: true});
    await response.text();
    const again = await harness.desktop.request('/api/download', {method: 'POST'});
    harness.emit({id: harness.calls[1].payload.streamId, done: true});
    await again.text();
});

test('rejected launches and subscription failures do not leave active streams', async () => {
    const harness = bridge({invoke: async () => { throw 'A download is already running'; }});
    await assert.rejects(harness.desktop.request('/api/download', {method: 'POST'}), /already running/);
    assert.equal(harness.removed(), 1);
    await assert.rejects(harness.desktop.request('/api/download', {method: 'POST'}), /already running/);
    assert.equal(harness.calls.length, 2);
    const unavailable = bridge({listen: async () => { throw new Error('Events unavailable'); }});
    await assert.rejects(unavailable.desktop.request('/api/download', {method: 'POST'}), /Events unavailable/);
    assert.equal(unavailable.calls.length, 0);
});

test('a late rejection from a completed stream cannot remove the next operation', async () => {
    let rejectFirst;
    let launches = 0;
    const harness = bridge({invoke: async (_, {streamId}, emit) => {
        if (++launches === 1) {
            emit({id: streamId, done: true});
            return new Promise((_, reject) => {rejectFirst = reject;});
        }
    }});
    const first = harness.desktop.request('/api/download', {method: 'POST'});
    await new Promise(resolve => setImmediate(resolve));
    const second = await harness.desktop.request('/api/download', {method: 'POST'});
    rejectFirst('First launch lost its response');
    await assert.rejects(first, /lost its response/);
    await assert.rejects(harness.desktop.request('/api/download', {method: 'POST'}), /already running/);
    const id = harness.calls[1].payload.streamId;
    harness.emit({id, done: true});
    await second.text();
});

test('consumer cancellation cancels the active download and releases listeners', async () => {
    const harness = bridge();
    const response = await harness.desktop.request('/api/download', {method: 'POST'});
    await response.body.cancel();
    assert.equal(harness.removed(), 1);
    assert.equal(harness.calls[1].command, 'desktop_request');
    assert.equal(harness.calls[1].payload.route, '/api/download/cancel');
    harness.emit({id: harness.calls[0].payload.streamId, data: {log: 'Late event'}});
    assert.equal(harness.removed(), 1);
});

test('abort and page shutdown reject pending readers instead of leaving them waiting', async () => {
    for (const mode of ['abort', 'pagehide']) {
        const harness = bridge();
        const abort = new AbortController();
        const response = await harness.desktop.request('/api/download', {method: 'POST', signal: abort.signal});
        const pending = response.body.getReader().read();
        if (mode === 'abort') abort.abort();
        else harness.lifecycle.get('pagehide')();
        await assert.rejects(pending, /aborted|closed/i);
        assert.equal(harness.subscriptions.size, 0);
        assert.equal(harness.calls[1].payload.route, '/api/download/cancel');
    }
});

test('an abort during listener setup prevents launching the worker and removes the late listener', async () => {
    let finishListen;
    const harness = bridge({listen: () => new Promise(resolve => { finishListen = resolve; })});
    const abort = new AbortController();
    const request = harness.desktop.request('/api/download', {method: 'POST', signal: abort.signal});
    abort.abort();
    finishListen();
    await assert.rejects(request, /aborted/i);
    assert.equal(harness.calls.length, 0);
    assert.equal(harness.subscriptions.size, 0);
});

test('an explicit stream transport error rejects the reader and releases its listener', async () => {
    const harness = bridge();
    const response = await harness.desktop.request('/api/setup/install-sync', {method: 'POST'});
    harness.emit({id: harness.calls[0].payload.streamId, error: 'Worker disconnected'});
    await assert.rejects(response.text(), /Worker disconnected/);
    assert.equal(harness.removed(), 1);
});

test('external links use the native opener and accept only http or https', async () => {
    const harness = bridge();
    await harness.desktop.openExternal('https://github.com/mkiera/FinFetcher');
    assert.equal(harness.calls[0].command, 'desktop_open_external');
    assert.equal(harness.calls[0].payload.url, 'https://github.com/mkiera/FinFetcher');
    await assert.rejects(harness.desktop.openExternal('file:///C:/app.exe'));
    await assert.rejects(harness.desktop.openExternal('javascript:alert(1)'));
    assert.equal(harness.calls.length, 1);
});
