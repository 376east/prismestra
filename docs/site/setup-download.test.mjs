import assert from 'node:assert/strict';
import test from 'node:test';
import { connectWindowsSetup, getWindowsSetup } from './setup-download.mjs';

const filename = 'Prismestra-Setup-0.2.0.0-x64.exe';
const url = `https://github.com/376east/prismestra/releases/download/v0.2.0/${filename}`;
const asset = { name: filename, browser_download_url: url };
const release = { tag_name: 'v0.2.0', draft: false, prerelease: false, assets: [asset] };
const from = (data) => async () => ({ ok: true, json: async () => data });

test('links directly to the x64 Setup of the latest stable release', async () => {
  const result = await getWindowsSetup((endpoint, options) => {
    assert.equal(endpoint, 'https://api.github.com/repos/376east/prismestra/releases/latest');
    assert.ok(options.signal instanceof AbortSignal);
    return from({ ...release, assets: [{ name: 'Prismestra-Sources-0.2.0.0.zip' }, asset] })();
  });
  assert.equal(result, url);
});

test('keeps the release page as fallback for unusable release data or request errors', async () => {
  for (const data of [
    { ...release, assets: [] },
    { ...release, tag_name: 'v0.3.0' },
    { ...release, prerelease: true },
    { ...release, draft: true },
    { ...release, assets: [{ ...asset, browser_download_url: 'https://example.com/setup.exe' }] },
  ]) assert.equal(await getWindowsSetup(from(data)), null);
  assert.equal(await getWindowsSetup(async () => ({ ok: false })), null);
  assert.equal(await getWindowsSetup(async () => { throw new Error('offline'); }), null);
});

test('download controls wait for a pending lookup and keep native links when ready', async () => {
  const fallback = 'https://github.com/376east/prismestra/releases/latest';
  for (const resolved of [url, null]) {
    let finish;
    let click;
    const calls = [];
    const link = { href: fallback, addEventListener: (_, listener) => { click = listener; } };
    const ready = connectWindowsSetup(link, {
      resolve: () => new Promise((resolve) => { finish = resolve; }),
      navigate: (href) => calls.push(href),
    });
    await click({ ctrlKey: true });
    let prevented = 0;
    const event = { preventDefault: () => { prevented++; } };
    const firstClick = click(event);
    assert.equal(prevented, 1);
    assert.deepEqual(calls, []);
    finish(resolved);
    assert.equal(await ready, resolved);
    await firstClick;
    assert.deepEqual(calls, [resolved ?? fallback]);
    assert.equal(link.href, resolved ?? fallback);
    await click(event);
    assert.equal(prevented, 1);
  }
});
