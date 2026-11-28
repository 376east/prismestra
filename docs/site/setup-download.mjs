const releases = 'https://github.com/376east/prismestra/releases';

export function renderWindowsDownloads(html, release) {
  const version = /^v(\d+\.\d+\.\d+)$/.exec(release?.tag_name)?.[1];
  if (!version || release.draft || release.prerelease || !Array.isArray(release.assets)) {
    throw new Error('Expected a stable Prismestra release');
  }

  function findSetup(filename) {
    const url = `${releases}/download/${release.tag_name}/${filename}`;
    return release.assets.some((asset) => asset?.name === filename && asset.browser_download_url === url)
      ? url : null;
  }

  const primary = findSetup(`Prismestra-Setup-${version}.0-x64.exe`);
  if (!primary) throw new Error(`Missing Windows Setup asset for ${release.tag_name}`);

  const offline = findSetup(`Prismestra-Setup-Offline-${version}.0-x64.exe`);
  const rendered = html.replace(`id="download" href="${releases}/latest"`, `id="download" href="${primary}"`);
  return offline ? rendered
    .replace('id="offline-download" hidden', `id="offline-download" href="${offline}"`)
    .replace('id="download-network" hidden', 'id="download-network"') : rendered;
}
