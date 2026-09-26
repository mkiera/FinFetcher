'use strict';

(() => {
    const streamRoutes = new Set(['/api/download', '/api/setup/install-sync', '/api/update/download']);
    const activeStreams = new Map();
    const encoder = new TextEncoder();
    const baseUrl = 'https://finfetcher.local';
    let sequence = 0;

    function asError(error) {
        return error instanceof Error ? error : new Error(String(error));
    }

    function native() {
        const tauri = window.__TAURI__;
        if (!tauri?.core?.invoke || !tauri?.event?.listen) {
            throw new Error('FinFetcher desktop services are unavailable. Restart the application.');
        }
        return tauri;
    }

    async function invoke(command, payload = {}) {
        try {
            return await native().core.invoke(command, payload);
        } catch (error) {
            throw asError(error);
        }
    }

    async function streamRequest(route, payload, signal) {
        if (activeStreams.has(route)) throw new Error('This operation is already running.');
        const tauri = native();
        const streamId = `${crypto.randomUUID()}-${++sequence}`;
        let controller;
        let unlisten;
        let stopped = false;
        let launched = false;
        let failure;
        const closeWindow = () => stop(new Error('The FinFetcher window was closed.'));

        const cleanup = () => {
            if (activeStreams.get(route) === closeWindow) activeStreams.delete(route);
            signal?.removeEventListener('abort', abort);
            if (unlisten) {
                const remove = unlisten;
                unlisten = null;
                remove();
            }
        };

        const stop = (error, cancelled = false) => {
            if (stopped) return;
            stopped = true;
            failure = error;
            cleanup();
            if (!cancelled) {
                if (error) controller.error(error);
                else controller.close();
            }
            if ((error || cancelled) && launched && route === '/api/download') {
                return invoke('desktop_request', {route: '/api/download/cancel', method: 'POST', payload: {}})
                    .catch(() => {});
            }
        };

        const abort = () => stop(new DOMException('The operation was aborted.', 'AbortError'));
        const body = new ReadableStream({
            start(value) { controller = value; },
            cancel() { return stop(null, true); },
        });
        activeStreams.set(route, closeWindow);
        signal?.addEventListener('abort', abort, {once: true});

        try {
            unlisten = await tauri.event.listen('finfetcher-stream', event => {
                const message = event.payload;
                if (stopped || message?.id !== streamId) return;
                if (message.error) {
                    stop(asError(message.error));
                    return;
                }
                if (message.data !== undefined) {
                    controller.enqueue(encoder.encode(`data: ${JSON.stringify(message.data)}\n\n`));
                }
                if (message.done) stop();
            });
            if (signal?.aborted) abort();
            if (stopped) {
                cleanup();
                if (failure) throw failure;
            } else {
                launched = true;
                await invoke('desktop_stream', {route, payload, streamId});
                if (failure) throw failure;
            }
            return new Response(body, {headers: {'Content-Type': 'text/event-stream'}});
        } catch (error) {
            launched = false;
            stop(asError(error));
            cleanup();
            throw asError(error);
        }
    }

    async function request(input, options = {}) {
        const url = new URL(input, baseUrl);
        if (url.origin !== baseUrl || (!url.pathname.startsWith('/api/') && url.pathname !== '/version.txt')) {
            throw new Error('Unsupported desktop request.');
        }
        if (options.signal?.aborted) throw new DOMException('The operation was aborted.', 'AbortError');
        const body = options.body ? JSON.parse(options.body) : {};
        if (!body || typeof body !== 'object' || Array.isArray(body)) {
            throw new Error('Desktop request data must be an object.');
        }
        const payload = {...Object.fromEntries(url.searchParams), ...body};
        const method = (options.method || 'GET').toUpperCase();
        if (streamRoutes.has(url.pathname)) return streamRequest(url.pathname, payload, options.signal);
        const result = await invoke('desktop_request', {route: url.pathname, method, payload});
        if (options.signal?.aborted) throw new DOMException('The operation was aborted.', 'AbortError');
        if (url.pathname === '/version.txt') {
            const version = typeof result === 'string' ? result : result?.version;
            if (typeof version !== 'string' || !version.trim()) throw new Error('Application version is unavailable.');
            return new Response(version, {headers: {'Content-Type': 'text/plain'}});
        }
        return new Response(JSON.stringify(result ?? null), {
            status: result?.error || result?.success === false ? 400 : 200,
            headers: {'Content-Type': 'application/json'},
        });
    }

    window.desktop = Object.freeze({
        get available() { return Boolean(window.__TAURI__?.core?.invoke && window.__TAURI__?.event?.listen); },
        request,
        async selectFolder() {
            return await invoke('desktop_select_folder');
        },
        async openExternal(input) {
            const url = new URL(input);
            if (!['http:', 'https:'].includes(url.protocol)) throw new Error('Unsupported external link.');
            return await invoke('desktop_open_external', {url: url.href});
        },
    });
    window.addEventListener('pagehide', () => {
        for (const close of [...activeStreams.values()]) close();
    });
    if (window.desktop.available) window.dispatchEvent(new Event('desktopready'));
})();
