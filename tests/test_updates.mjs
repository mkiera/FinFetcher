import assert from 'node:assert/strict';
import test from 'node:test';
import {frontend, tick} from './frontend_harness.mjs';

const installer = name => ({name: 'FinFetcher-Setup.exe', url: `https://github.com/mkiera/FinFetcher/releases/download/${name}/FinFetcher-Setup.exe`, is_installer: true});

async function withTimers(settings) {
    const intervals = [];
    const app = await frontend(async (_, payload) => {
        if (payload.route === '/api/update/settings' && payload.method === 'GET') return settings;
    });
    app.context.setInterval = callback => { intervals.push(callback); return intervals.length; };
    return {app, intervals};
}

test('older releases are never auto-selected and need confirmation as a downgrade', async () => {
    const app = await frontend(async (_, payload) => {
        if (payload.route === '/api/update/releases') {
            return {current_version: '1.3.0-beta.1', releases: [
                {version: '1.2.9', is_downgrade: true, is_current: false, exe_asset: installer('v1.2.9')},
                {version: '1.2.8', is_downgrade: true, is_current: false, exe_asset: installer('v1.2.8')},
            ]};
        }
        if (payload.route === '/api/update/artifacts') {
            return {current_version: '1.3.0-beta.1', artifacts: [
                {branch: 'fix-x', sha: 'abc1234', run_id: 5, is_current: false, exe_asset: {name: 'FinFetcher-Setup.zip', url: 'https://nightly.link/mkiera/FinFetcher/actions/runs/5/FinFetcher-Setup.zip', is_installer: true}},
            ]};
        }
    });
    const button = app.elements.get('installSelectedBtn');
    await app.context.fetchReleases('stable');
    assert.equal(button.disabled, true);
    assert.equal(button.textContent, 'Update');
    assert.equal(app.run('selectedRelease'), null);

    const [row] = app.elements.get('releasesList').children;
    await row.fire('click');
    assert.equal(button.disabled, false);
    assert.equal(button.textContent, 'Downgrade to v1.2.9');

    const prompts = [];
    let answer = false;
    app.context.confirm = message => { prompts.push(message); return answer; };
    await app.context.installSelectedVersion();
    await tick();
    assert.equal(prompts.length, 1);
    assert.match(prompts[0], /v1\.2\.9 is older than the version you are running/);
    assert.equal(app.calls.some(call => call.command === 'desktop_stream'), false);

    answer = true;
    await app.context.installSelectedVersion();
    await tick();
    const download = app.calls.find(call => call.command === 'desktop_stream');
    assert.equal(download.payload.payload.url, installer('v1.2.9').url);

    await app.context.fetchReleases('alpha');
    assert.equal(button.textContent, 'Update to fix-x@abc1234');
});

test('a newer release is still auto-selected as an update without a prompt', async () => {
    const app = await frontend(async (_, payload) => {
        if (payload.route === '/api/update/releases') {
            return {current_version: '1.2.9', releases: [
                {version: '1.3.0', is_downgrade: false, is_current: false, exe_asset: installer('v1.3.0')},
                {version: '1.2.9', is_downgrade: false, is_current: true, exe_asset: installer('v1.2.9')},
            ]};
        }
    });
    await app.context.fetchReleases('stable');
    assert.equal(app.elements.get('installSelectedBtn').textContent, 'Update to v1.3.0');
    app.context.confirm = () => { throw new Error('No prompt for an update'); };
    await app.context.installSelectedVersion();
    await tick();
    assert.ok(app.calls.some(call => call.command === 'desktop_stream'));
});

test('the hourly update check runs only while automatic checks are on', async () => {
    const {app, intervals} = await withTimers({auto_check_updates: false, update_channel: 'stable', can_self_update: true});
    assert.equal(app.run('updateCheckTimer'), null);

    await app.context.manualCheckForUpdates();
    const manual = app.calls.findLast(call => call.payload.route === '/api/update/check');
    assert.equal(manual.payload.payload.manual, 'true');
    assert.equal(app.run('updateCheckTimer'), null);
    await app.context.refreshReleases();
    assert.equal(app.run('updateCheckTimer'), null);
    assert.equal(intervals.length, 0);

    const toggle = app.elements.get('autoUpdateToggle');
    toggle.checked = true;
    await toggle.fire('change');
    assert.notEqual(app.run('updateCheckTimer'), null);
    assert.equal(intervals.length, 1);
    await intervals[0]();
    const scheduled = app.calls.findLast(call => call.payload.route === '/api/update/check');
    assert.equal(scheduled.payload.payload.force, 'true');
    assert.equal(scheduled.payload.payload.manual, undefined);

    toggle.checked = false;
    await toggle.fire('change');
    assert.equal(app.run('updateCheckTimer'), null);
    const saved = app.calls.findLast(call => call.payload.route === '/api/update/settings' && call.payload.method === 'POST');
    assert.equal(saved.payload.payload.auto_check_updates, false);
});

test('startup with automatic checks on starts the hourly timer once', async () => {
    const {app, intervals} = await withTimers({auto_check_updates: true, update_channel: 'stable', can_self_update: true});
    await app.context.showMainApp();
    await tick();
    assert.equal(intervals.length, 1);
    const startup = app.calls.findLast(call => call.payload.route === '/api/update/check');
    assert.equal(startup.payload.payload.manual, undefined);
});

test('a recovered settings file is reported in the log once', async () => {
    const notice = 'The settings file was damaged, so FinFetcher started with default settings. The damaged file was kept as C:\\Data\\config.json.corrupt-20261030-120000.';
    const app = await frontend(async (_, payload) => {
        if (payload.route === '/api/update/settings' && payload.method === 'GET') {
            return {auto_check_updates: false, update_channel: 'stable', can_self_update: true, settings_notice: notice};
        }
    });
    await app.context.loadUpdateSettings();
    assert.equal(app.logs().split(notice).length - 1, 1);
    assert.match(app.run('lastError'), /settings file was damaged/);
});
