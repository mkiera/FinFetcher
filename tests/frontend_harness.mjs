import fs from 'node:fs';
import vm from 'node:vm';

const html = fs.readFileSync(new URL('../index.html', import.meta.url), 'utf8');

export const tick = () => new Promise(resolve => setImmediate(resolve));

export async function frontend(invoke = async () => undefined) {
    const elements = new Map();
    const controls = [];
    const events = new Map();
    const subscriptions = new Map();
    const calls = [];
    const alerts = [];
    const timers = new Map();
    let timerId = 0;
    let listenerId = 0;
    let document;

    class Element {
        constructor(tagName = 'div', attributes = '') {
            this.tagName = tagName;
            this.className = attributes.match(/class="([^"]*)"/)?.[1] || '';
            this.id = attributes.match(/id="([^"]*)"/)?.[1] || '';
            this.value = attributes.match(/value="([^"]*)"/)?.[1] || '';
            this.checked = /\bchecked\b/.test(attributes);
            this.disabled = /\bdisabled\b/.test(attributes);
            this.textContent = '';
            this.style = {};
            this.dataset = {};
            for (const [, name, value] of attributes.matchAll(/\bdata-([\w-]+)="([^"]*)"/g)) {
                this.dataset[name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = value;
            }
            this.children = [];
            this.listeners = new Map();
            this.options = [];
            this.classList = {
                contains: name => this.className.split(/\s+/).includes(name),
                add: (...names) => { for (const name of names) this.classList.toggle(name, true); },
                remove: (...names) => { for (const name of names) this.classList.toggle(name, false); },
                toggle: (name, force) => {
                    const classes = new Set(this.className.split(/\s+/).filter(Boolean));
                    const enabled = force ?? !classes.has(name);
                    if (enabled) classes.add(name);
                    else classes.delete(name);
                    this.className = [...classes].join(' ');
                    return enabled;
                },
            };
        }
        addEventListener(name, callback) { this.listeners.set(name, callback); }
        fire(name) {
            const handler = this.listeners.get(name);
            if (!handler) throw new Error(`No ${name} listener on ${this.id || this.dataset[name] || this.tagName}`);
            return handler({target: this, currentTarget: this, preventDefault() {}});
        }
        appendChild(element) {
            this.children.push(element);
            if (this.tagName === 'select' && element.tagName === 'option') this.options.push(element);
        }
        replaceChildren(...children) { this.children = children; }
        focus() { document.activeElement = this; }
        pause() { this.paused = true; }
        querySelectorAll(selector) {
            if (this.id === 'destinationModal') return destinationButtons.filter(button => !selector.includes(':disabled') || !button.disabled);
            if (this.id === 'subtitleOptions') return ['subtitleLangs', 'subtitlesAutoToggle', 'embedSubtitlesToggle'].map(id => elements.get(id));
            if (this.id === 'sponsorblockOptions') return sponsorBoxes;
            return [];
        }
        set innerHTML(value) {
            this.children = [];
            this.options = [...value.matchAll(/<option\b([^>]*)>([^<]*)<\/option>/g)]
                .map(([, attributes, text]) => Object.assign(new Element('option', attributes), {textContent: text}));
            if (this.tagName === 'select') this.value = this.options[0]?.value || '';
            this.markup = value;
        }
        get innerHTML() { return this.markup || ''; }
    }

    for (const [, tagName, attributes] of html.matchAll(/<([a-z][\w-]*)\b([^>]*)>/g)) {
        const element = new Element(tagName, attributes);
        controls.push(element);
        if (element.id) elements.set(element.id, element);
    }
    for (const [, attributes, content] of html.matchAll(/<select\b([^>]*)>([\s\S]*?)<\/select>/g)) {
        const id = attributes.match(/id="([^"]+)"/)?.[1];
        if (id) elements.get(id).innerHTML = content;
    }
    const sponsorBoxes = controls.filter(element => element.tagName === 'input' && ['sponsor', 'selfpromo', 'interaction', 'intro', 'outro', 'preview', 'music_offtopic'].includes(element.value));
    const destinationButtons = controls.filter(element => element.dataset.click === 'chooseDownloadDestination');
    const modeCards = controls.filter(element => element.dataset.mode);
    const channelTabs = controls.filter(element => element.dataset.channel);
    const sectionTabs = controls.filter(element => element.dataset.section);
    const chevron = Object.assign(new Element(), {textContent: '▼'});
    const extra = new Map([['.setup-note', new Element()], ['.advanced-options', new Element()], ['.advanced-header .chevron', chevron]]);
    document = {
        activeElement: null,
        fullscreenElement: null,
        getElementById: id => elements.get(id) || null,
        createElement: name => new Element(name),
        querySelector: selector => selector.startsWith('#') ? elements.get(selector.slice(1)) : extra.get(selector),
        querySelectorAll: selector => {
            const event = selector.match(/^\[data-(click|change|input)\]$/)?.[1];
            if (event) return controls.filter(element => element.dataset[event]);
            if (selector === '.option-card') return modeCards;
            if (selector === '.settings-tab[data-channel]') return channelTabs;
            if (selector === '.settings-tab[data-section]') return sectionTabs;
            if (selector.startsWith('#sponsorblockCategories')) return sponsorBoxes;
            if (selector === '#destinationModal button') return destinationButtons;
            if (selector === '.release-row.selected') return elements.get('releasesList').children.filter(element => element.classList.contains('selected'));
            throw new Error(`Unsupported test selector: ${selector}`);
        },
        addEventListener: (name, callback) => events.set(name, [...(events.get(name) || []), callback]),
    };
    const emit = payload => { for (const callback of [...subscriptions.values()]) callback({payload}); };
    const defaults = {
        '/api/setup/check': {installed: true},
        '/version.txt': '2.0.0-beta.1',
        '/api/integrations/flipperclipper': {installed: true},
        '/api/update/settings': {auto_check_updates: false, update_channel: 'stable', can_self_update: true},
        '/api/update/check': {available: false},
        '/api/settings': {},
    };
    const window = {
        __TAURI__: {
            core: {invoke: async (command, payload = {}) => {
                calls.push({command, payload});
                const result = await invoke(command, payload, emit);
                if (result !== undefined) return result;
                return defaults[payload.route] ?? {success: true};
            }},
            event: {listen: async (_, callback) => {
                const id = ++listenerId;
                subscriptions.set(id, callback);
                return () => subscriptions.delete(id);
            }},
        },
        dispatchEvent: () => {},
        addEventListener: (name, callback) => events.set(name, [...(events.get(name) || []), callback]),
    };
    const context = vm.createContext({window, document, URL, URLSearchParams, Response, ReadableStream,
        TextEncoder, TextDecoder, DOMException, crypto: globalThis.crypto, Event,
        console: {log() {}, warn() {}, error() {}}, alert: message => alerts.push(message),
        setTimeout: (callback, delay) => { const id = ++timerId; timers.set(id, {callback, delay}); return id; },
        clearTimeout: id => timers.delete(id),
        setInterval: () => ++timerId, clearInterval: () => {},
        navigator: {clipboard: {writeText: async () => {}}},
    });
    for (const path of ['desktop.js', 'script.js']) {
        vm.runInContext(fs.readFileSync(new URL(`../${path}`, import.meta.url), 'utf8'), context, {filename: path});
    }
    await tick();
    return {context, document, calls, emit, elements, controls, subscriptions, alerts, events, timers,
        run: source => vm.runInContext(source, context),
        key: event => { for (const callback of events.get('keydown') || []) callback(event); },
        logs: () => elements.get('logContainer').children.map(element => element.textContent).join('\n')};
}
