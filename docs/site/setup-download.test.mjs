import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { renderWindowsDownloads } from './setup-download.mjs';

const releases = 'https://github.com/376east/prismestra/releases';
const pages = ['./index.html', '../devices/index.html'].map((path) =>
  readFileSync(new URL(path, import.meta.url), 'utf8'));

function setupAsset(tag, kind = '') {
  const name = `Prismestra-Setup-${kind}${tag.slice(1)}.0-x64.exe`;
  return { name, browser_download_url: `${releases}/download/${tag}/${name}` };
}

test('links directly to the regular Setup and keeps other assets in Release files', () => {
  for (const tag_name of ['v0.2.0', 'v0.3.0']) {
    const primary = setupAsset(tag_name);
    const offline = setupAsset(tag_name, 'Offline-');
    const assets = tag_name === 'v0.2.0' ? [primary] : [offline, primary];
    for (const page of pages) {
      const html = renderWindowsDownloads(page, { tag_name, assets });
      assert.ok(html.includes(`id="download" href="${primary.browser_download_url}"`));
      assert.ok(html.includes(`id="manual-download" href="${releases}/latest"`));
      assert.ok(!html.includes(offline.browser_download_url));
    }
  }
});

test('rejects an invalid, unpublished or incomplete latest release', () => {
  const tag_name = 'v0.3.0';
  const primary = setupAsset(tag_name);
  const release = { tag_name, assets: [primary] };
  for (const invalid of [
    null,
    { ...release, tag_name: 'v0.3.0-preview.1' },
    { ...release, prerelease: true },
    { ...release, draft: true },
  ]) assert.throws(() => renderWindowsDownloads(pages[0], invalid), /Expected a stable/);

  for (const assets of [[], [{ ...primary, browser_download_url: 'https://example.com/setup.exe' }]]) {
    assert.throws(() => renderWindowsDownloads(pages[0], { ...release, assets }), /Missing Windows Setup asset/);
  }
});
